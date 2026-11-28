//! Carries out content requests (SPEC §content) and follows feeds (NETWORK 4.4) on a
//! background thread, so the frame loop only ever submits requests and
//! collects completions and feed events without blocking. Blocking waits are
//! for headless and command-line use.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use terse_core::page::{Layouter, PageInput};
use terse_core::system::CONTROLLERS;
use terse_core::machines::failure;
use terse_core::{
    Completion, Controller, Driver, Event, FrameInput, FrameResult, Machine, Outcome, ProgramId, Request, RequestKind,
    Script, Snapshot, StartFailure, System, SystemFrame, Who,
};

use crate::identity::{FeedKey, valid_feed_key};
use crate::node::{Done, Draft, FeedEvent, Node, Token};
use crate::records::{Owner, Saved};

/// How long the worker sleeps between reconnection checks while following
/// feeds without a connection.
const IDLE_WAIT: Duration = Duration::from_millis(20);
/// The wait for work with no relays but a subscription file to look at.
const SUBSCRIPTIONS_WAIT: Duration = Duration::from_millis(250);

/// Feed operations for the worker's node.
#[derive(Clone, Debug)]
pub enum FeedCommand {
    Subscribe(FeedKey),
    Unsubscribe(FeedKey),
    /// Reported as `FeedEvent::Published` or `FeedEvent::PublishFailed`.
    Publish(Draft),
}

/// Requests carry a tag that their completions carry back: the machine
/// they came from, for a host that runs several (SPEC §machines). 0 for one.
enum Work {
    /// With the owner of the machine's named records (SPEC §content.records), if known.
    Content(u32, Request, Option<Owner>),
    /// A request the user has allowed (SPEC §content.feeds).
    Consented(u32, Request),
    Feed(FeedCommand),
    /// A whole blob (an image a shell starts, SPEC §machines.calls), completed as a
    /// get with handle 0.
    Fetch(u32, [u8; 32]),
    /// The current version of a program (`start_program`, SPEC §machines.calls),
    /// completed like `Fetch`: not found if the host holds no such program.
    /// A built-in (the all-zero key) is looked for among these images.
    Program(u32, ProgramId, Vec<[u8; 32]>),
    /// Store a snapshot file in a slot of the owner's snapshots (SPEC
    /// §machines.snapshots); reported in [`ContentService::take_snapshot_results`].
    SaveSnapshot(Owner, u8, Vec<u8>),
    /// Read a slot's snapshot and get its image, for machine `tag`.
    Restore(u32, Owner, u8),
    DeleteSnapshot(Owner, u8),
    /// The blobs an image lists (SPEC §image.blobs), fetched if not held.
    Listed(Vec<terse_core::image::ListedBlob>),
}

/// How the host answers the requests that need the user's consent
/// (subscribe, unsubscribe, publish; SPEC §content.feeds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consent {
    /// Ask the user: the request waits in [`ContentService::prompt`] until
    /// [`ContentService::answer`].
    Ask,
    Allow,
    Deny,
}

impl Consent {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ask" => Some(Consent::Ask),
            "allow" => Some(Consent::Allow),
            "deny" => Some(Consent::Deny),
            _ => None,
        }
    }
}

/// What a program asks for, as the host's prompt describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// Subscribe to a feed; `hints`: the hint list of its address (NETWORK
    /// 4.9), where the host will look for it if its own relays do not hold
    /// it, or empty.
    Subscribe { feed: FeedKey, hints: String },
    Unsubscribe(FeedKey),
    /// Publish an article, or with `document` a document (`size` bytes),
    /// with this title.
    Publish { title: String, size: u32, document: bool },
    /// Publish a profile with this display name; `listed`: findable by
    /// search.
    SetProfile { name: String, listed: bool },
    /// Give a feed a nickname (empty: remove it).
    SetNickname { feed: FeedKey, name: String },
}

/// A request waiting for the user's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    /// The tag the request was submitted with (its machine).
    pub tag: u32,
    pub handle: u8,
    pub asked: Asked,
}

enum Finished {
    Done(u32, Completion),
    Ask(Prompt, Request),
    /// A snapshot was stored in a slot (or not).
    Saved(u8, bool),
    /// A snapshot was deleted (so that waiting for everything waits for it).
    Deleted,
    /// The snapshot and image for a restored machine, or why not.
    Restored(u32, Result<(terse_core::Image, Snapshot), StartFailure>),
}

pub struct ContentService {
    jobs: Sender<Work>,
    finished: Receiver<Finished>,
    events: Receiver<FeedEvent>,
    /// Requests submitted and not yet returned by `take_finished`/`wait_finished`
    /// (nor waiting for the user's answer).
    outstanding: usize,
    /// Requests waiting for the user's answer, the first one shown.
    prompts: VecDeque<(Prompt, Request)>,
    /// Requests the user refused, to complete with the next ones.
    refused: Vec<(u32, Completion)>,
    /// The node has a relay and subscriptions.
    following: bool,
    /// Snapshot results not yet taken: slots stored (or not), machines'
    /// snapshots and images.
    saved: Vec<(u8, bool)>,
    restored: Vec<(u32, Result<(terse_core::Image, Snapshot), StartFailure>)>,
    /// The machine's reading profile (SPEC §profiles): requests
    /// outside it complete with status Refused without asking.
    reading: bool,
}

/// Whether a request is one the reading profile carries out (SPEC §profiles):
/// get, subscriptions, timeline, names, identity,
/// subscribe and unsubscribe. Every other operation (put, publish, find,
/// set profile, set nickname, programs, save, load, delete, records) is
/// optional and completes with status 05h on a host that offers only the
/// reading profile.
pub fn in_reading_profile(kind: &RequestKind) -> bool {
    matches!(
        kind,
        RequestKind::Get { .. }
            | RequestKind::Subscriptions { .. }
            | RequestKind::Timeline { .. }
            | RequestKind::Names { .. }
            | RequestKind::Identity
            | RequestKind::Subscribe { .. }
            | RequestKind::Unsubscribe { .. }
    )
}

/// What the worker needs to turn a `Done` into a completion.
enum Job {
    Get { handle: u8, offset: u32, length: u16 },
    Put { handle: u8 },
    /// A program's publish or set profile (SPEC §content.feeds).
    Publish { handle: u8 },
    /// A program's find, with its buffer size.
    Find { handle: u8, length: u16 },
    /// `FeedCommand::Publish`, reported as a feed event.
    FeedPublish,
    /// A whole blob for `fetch`.
    Fetch,
    /// The relays asked for a program the host does not hold (NETWORK
    /// 4.8); its image is fetched next.
    Lookup(ProgramId),
    /// The image of a snapshot being restored (SPEC §machines.snapshots).
    Restore(Box<Snapshot>),
}

impl ContentService {
    /// A service that refuses every request needing consent (for headless
    /// runs, where nobody can answer).
    pub fn start(node: Node) -> Self {
        Self::start_with_consent(node, Consent::Deny)
    }

    pub fn start_with_consent(node: Node, consent: Consent) -> Self {
        let following = node.following();
        let (jobs, job_rx) = mpsc::channel();
        let (done_tx, finished) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        thread::Builder::new()
            .name("terse-content".into())
            .spawn(move || worker(node, consent, job_rx, done_tx, event_tx))
            .expect("spawn content thread");
        ContentService {
            jobs,
            finished,
            events,
            outstanding: 0,
            prompts: VecDeque::new(),
            refused: Vec::new(),
            following,
            saved: Vec::new(),
            restored: Vec::new(),
            reading: false,
        }
    }

    /// Offers only the machine's reading profile (`terse run --profile
    /// reading`): every request outside it ([`in_reading_profile`]) is
    /// completed with status Refused, without asking and without the node.
    pub fn set_reading_profile(&mut self, on: bool) {
        self.reading = on;
    }

    /// A request the reading profile refuses: completed with the next
    /// completions. Returns false if the request goes on to the worker.
    fn refuse_outside_profile(&mut self, tag: u32, r: &Request) -> bool {
        let refuse = self.reading && !in_reading_profile(&r.kind);
        if refuse {
            self.refused.push((tag, Completion { handle: r.handle, outcome: Outcome::Refused }));
        }
        refuse
    }

    /// The request the user is asked about now, if any (one at a time, the
    /// others queued in handle order).
    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompts.front().map(|(p, _)| p)
    }

    /// How many requests wait for an answer, the one shown included.
    pub fn prompts_waiting(&self) -> usize {
        self.prompts.len()
    }

    /// The user's answer to the request shown by [`Self::prompt`]: carried
    /// out if allowed, completed with status Refused if not.
    pub fn answer(&mut self, allow: bool) {
        let Some((prompt, request)) = self.prompts.pop_front() else { return };
        if allow {
            self.outstanding += 1;
            let _ = self.jobs.send(Work::Consented(prompt.tag, request));
        } else {
            self.refused.push((prompt.tag, Completion { handle: prompt.handle, outcome: Outcome::Refused }));
        }
    }

    /// Drops the prompts of requests submitted with `tag` (a machine that
    /// ceased to exist, SPEC §machines.calls). Their requests are never completed.
    pub fn cancel(&mut self, tag: u32) {
        self.prompts.retain(|(p, _)| p.tag != tag);
        self.refused.retain(|(t, _)| *t != tag);
    }

    fn receive(&mut self, f: Finished, done: &mut Vec<(u32, Completion)>) {
        self.outstanding -= 1;
        match f {
            Finished::Done(tag, c) => done.push((tag, c)),
            Finished::Saved(slot, ok) => self.saved.push((slot, ok)),
            Finished::Deleted => {}
            Finished::Restored(tag, r) => self.restored.push((tag, r)),
            Finished::Ask(prompt, request) => {
                // Queued in (tag, handle) order among those not yet shown.
                let key = (prompt.tag, prompt.handle);
                let at = (1..self.prompts.len().max(1))
                    .find(|&i| (self.prompts[i].0.tag, self.prompts[i].0.handle) > key)
                    .unwrap_or(self.prompts.len());
                self.prompts.insert(at, (prompt, request));
            }
        }
    }

    /// Whether the node follows feeds through a relay.
    pub fn following(&self) -> bool {
        self.following
    }

    /// Whether a new feed entry was stored since the last call (the
    /// feeds-updated event, SPEC §content.events); never blocks. Consumes the feed
    /// events so far, like `take_feed_events`.
    pub fn feeds_updated(&mut self) -> bool {
        self.take_feed_events()
            .iter()
            .any(|e| matches!(e, FeedEvent::Entry { .. } | FeedEvent::ProgramDownloaded { .. } | FeedEvent::SubscriptionsChanged))
    }

    /// Waits until every feed is caught up (NETWORK 4.3) and its automatic
    /// downloads are done. Fails if the relay cannot be reached or `timeout`
    /// passes first.
    pub fn wait_caught_up(&mut self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.wait_feed_event(deadline.saturating_duration_since(Instant::now())) {
                Some(FeedEvent::Settled) => return Ok(()),
                Some(FeedEvent::Unreachable) => return Err("cannot reach any relay".into()),
                Some(_) => {}
                None => return Err(format!("not caught up after {} s", timeout.as_secs())),
            }
        }
    }

    pub fn submit(&mut self, requests: Vec<Request>) {
        self.submit_tagged(0, requests);
    }

    /// Submits requests whose completions (and prompts) carry `tag`.
    pub fn submit_tagged(&mut self, tag: u32, requests: Vec<Request>) {
        for r in requests {
            if self.refuse_outside_profile(tag, &r) {
                continue;
            }
            self.outstanding += 1;
            // The worker only stops when this service is dropped.
            let _ = self.jobs.send(Work::Content(tag, r, None));
        }
    }

    /// Submits the requests `machine` made, with `tag`; its named records
    /// (SPEC §content.records) are those of its identity, else of its image.
    pub fn submit_machine(&mut self, tag: u32, machine: &mut Machine) {
        let owner = Owner::of(machine);
        let requests = machine.take_content_requests();
        self.submit_owned(tag, owner, requests);
    }

    /// Submits `requests` with `tag`; their named records (SPEC §content.records) are
    /// `owner`'s (for a content device kept outside the machine).
    pub fn submit_owned(&mut self, tag: u32, owner: Owner, requests: Vec<Request>) {
        for r in requests {
            if self.refuse_outside_profile(tag, &r) {
                continue;
            }
            self.outstanding += 1;
            let _ = self.jobs.send(Work::Content(tag, r, Some(owner.clone())));
        }
    }

    /// Fetches the image of the current version of `program` the host
    /// holds (a built-in among `builtins` for the all-zero key); completed
    /// as [`Self::fetch`] is.
    pub fn fetch_program(&mut self, tag: u32, program: &ProgramId, builtins: &[[u8; 32]]) {
        self.outstanding += 1;
        let _ = self.jobs.send(Work::Program(tag, program.clone(), builtins.to_vec()));
    }

    /// Fetches the whole blob `id` (from the store, else the relays);
    /// completed, with `tag`, as handle 0 with `Outcome::Got`, `NotFound`
    /// or `Failed`.
    /// Gets the blobs `image` lists that the store does not hold (SPEC
    /// §image.blobs), without waiting and without reporting: a host calls it
    /// when it starts a machine of the image. A shell's start, `start_program`
    /// and restore ([`Self::fetch`], [`Self::fetch_program`]) do it for the
    /// image they get.
    pub fn fetch_listed(&mut self, image: &terse_core::Image) {
        let listed = image.blob_list();
        if !listed.is_empty() {
            let _ = self.jobs.send(Work::Listed(listed));
        }
    }

    pub fn fetch(&mut self, tag: u32, id: [u8; 32]) {
        self.outstanding += 1;
        let _ = self.jobs.send(Work::Fetch(tag, id));
    }

    /// Stores `snapshot` in slot `slot` of `owner`'s snapshots (SPEC
    /// §machines.snapshots); the result comes from [`Self::take_snapshot_results`].
    pub fn save_snapshot(&mut self, owner: Owner, slot: u8, snapshot: &Snapshot) {
        self.outstanding += 1;
        let _ = self.jobs.send(Work::SaveSnapshot(owner, slot, snapshot.encode()));
    }

    /// Reads slot `slot` of `owner`'s snapshots and gets its image (store,
    /// else the relays), for machine `tag`; the result comes from
    /// [`Self::take_snapshot_results`].
    pub fn restore_snapshot(&mut self, tag: u32, owner: Owner, slot: u8) {
        self.outstanding += 1;
        let _ = self.jobs.send(Work::Restore(tag, owner, slot));
    }

    pub fn delete_snapshot(&mut self, owner: Owner, slot: u8) {
        self.outstanding += 1;
        let _ = self.jobs.send(Work::DeleteSnapshot(owner, slot));
    }

    /// Snapshots stored (slot, whether it was) and restores ready (tag, the
    /// image and snapshot or why not) since the last call; collected by
    /// `take_finished_tagged` and `wait_finished_tagged`.
    #[allow(clippy::type_complexity)]
    pub fn take_snapshot_results(&mut self) -> (Vec<(u8, bool)>, Vec<(u32, Result<(terse_core::Image, Snapshot), StartFailure>)>) {
        (std::mem::take(&mut self.saved), std::mem::take(&mut self.restored))
    }

    pub fn feed(&self, command: FeedCommand) {
        let _ = self.jobs.send(Work::Feed(command));
    }

    /// Feed events so far; never blocks.
    pub fn take_feed_events(&mut self) -> Vec<FeedEvent> {
        self.events.try_iter().collect()
    }

    /// The next feed event, waiting up to `timeout`.
    pub fn wait_feed_event(&mut self, timeout: Duration) -> Option<FeedEvent> {
        self.events.recv_timeout(timeout).ok()
    }

    pub fn outstanding(&self) -> usize {
        self.outstanding
    }

    /// Completions finished so far; never blocks.
    pub fn take_finished(&mut self) -> Vec<Completion> {
        self.take_finished_tagged().into_iter().map(|(_, c)| c).collect()
    }

    /// Completions finished so far, with their tags; never blocks.
    pub fn take_finished_tagged(&mut self) -> Vec<(u32, Completion)> {
        let mut done = std::mem::take(&mut self.refused);
        while let Ok(f) = self.finished.try_recv() {
            self.receive(f, &mut done);
        }
        done
    }

    /// Blocks until every submitted request has finished (each is bounded by
    /// the relay timeout) or waits for the user's answer. For headless runs
    /// that must not depend on timing.
    pub fn wait_finished(&mut self) -> Vec<Completion> {
        self.wait_finished_tagged().into_iter().map(|(_, c)| c).collect()
    }

    /// [`Self::wait_finished`], with the completions' tags.
    pub fn wait_finished_tagged(&mut self) -> Vec<(u32, Completion)> {
        let mut done = std::mem::take(&mut self.refused);
        while self.outstanding > 0 {
            match self.finished.recv() {
                Ok(f) => self.receive(f, &mut done),
                Err(_) => break,
            }
        }
        done
    }
}

/// Runs a machine the reproducible headless way (`terse run`, `terse-host
/// --screenshot`): scripted events and controllers (`pad` lines), Unix time
/// 0, random bytes 00h, and every content request completed at the start of the
/// next frame. A feeds-updated event is delivered in the frame after the one
/// in which the host stored a new entry; entries stored before the first
/// frame (the initial sync) are not announced. The page (device 7) is laid
/// out between frames, with the reference layout unless a layouter is given,
/// so its height event arrives in the frame after the change; a pointer
/// press over anchored text is followed by its anchor event.
pub struct Headless {
    content: Option<ContentService>,
    script: Script,
    completions: Vec<Completion>,
    feeds_updated: bool,
    /// The page's events (SPEC §page.events), from the reference layout unless the
    /// host gives its own layouter.
    page: PageInput,
    /// Completions as a windowed host gives them ([`Self::realtime`]).
    realtime: bool,
}

impl Headless {
    pub fn new(content: Option<ContentService>, script: Script) -> Self {
        Headless { content, script, completions: Vec::new(), feeds_updated: false, page: PageInput::default(), realtime: false }
    }

    /// Completes requests as a windowed host does (`terse-host`'s frame
    /// loop): each frame gets the completions that finished before it
    /// began, without waiting for the others, so how many frames a request
    /// takes depends on how long the host takes to carry it out (`terse
    /// perf`). The caller paces the frames.
    pub fn realtime(mut self) -> Self {
        self.realtime = true;
        self
    }

    /// Lays the page out with `layouter` instead of the reference layout.
    pub fn with_layouter(mut self, layouter: Box<dyn Layouter + Send>) -> Self {
        self.page = PageInput::new(layouter);
        self
    }

    /// The content service, if the machine has the content device.
    pub fn content(&self) -> Option<&ContentService> {
        self.content.as_ref()
    }

    /// The content service, to go on with another script.
    pub fn into_content(self) -> Option<ContentService> {
        self.content
    }

    /// The layouter, holding the latest layout of the page.
    pub fn page_layouter(&self) -> &dyn Layouter {
        self.page.layouter()
    }

    /// Runs the machine's next frame, then carries out the requests it made.
    pub fn frame(&mut self, machine: &mut Machine) -> FrameResult {
        let mut events = self.page.events(machine.page(), self.script.events(machine.frame()));
        if std::mem::take(&mut self.feeds_updated) {
            events.push(Event::feeds_updated());
        }
        if self.realtime
            && let Some(content) = &mut self.content
        {
            self.completions.extend(content.take_finished());
        }
        let completions = std::mem::take(&mut self.completions);
        let controllers = self.script.controllers(machine.frame());
        // In real time the clock is the real one (programs/conform-perf counts frames against it).
        let unix_time = if self.realtime { now() } else { 0 };
        let input = FrameInput { events: &events, completions: &completions, controllers, unix_time, ..Default::default() };
        let result = machine.run_frame(&input);
        // The height of a changed page is reported at the next frame.
        self.page.after_frame(machine.page());
        if let Some(content) = &mut self.content {
            content.submit_machine(0, machine);
            if !self.realtime {
                self.completions = content.wait_finished();
            }
            self.feeds_updated = content.feeds_updated();
        }
        result
    }
}

/// An image a shell started: by ID, or a program and the built-ins.
enum Wanted {
    Image([u8; 32]),
    Program(ProgramId, Vec<[u8; 32]>),
}

/// One machine's part of a [`HeadlessSystem`].
struct HeadlessMachine {
    page: PageInput,
    completions: Vec<Completion>,
    feeds_updated: bool,
}

/// Runs a [`System`] (a shell and the machines it starts, SPEC §machines) the
/// reproducible headless way, as [`Headless`] runs one machine: the script's
/// events are the host's input, routed by the system; each machine has its
/// own page events (laid out between frames), completions and feeds-updated
/// events, all from the one content service; every content request and
/// every image a shell starts is finished before the next frame, so a
/// machine started in frame N is loaded at the start of frame N + 1.
pub struct HeadlessSystem {
    content: Option<ContentService>,
    script: Script,
    machines: HashMap<u32, HeadlessMachine>,
    /// Images wanted in the frame just run: by ID, or as a program.
    wanted: Vec<(u32, Wanted)>,
    /// Images to deliver at the start of the next frame.
    images: Vec<(u32, Result<terse_core::Image, StartFailure>)>,
    /// Snapshots (with their images) to deliver at the start of the next
    /// frame (SPEC §machines.snapshots).
    restores: Vec<(u32, Result<(terse_core::Image, Snapshot), StartFailure>)>,
    /// Makes the layouter of each machine's page (default: the reference
    /// layout).
    layouter: Box<dyn Fn(u32) -> Box<dyn Layouter + Send>>,
    /// The inputs of the frames run since the last `take_recorded`, if
    /// recording (`terse run --record`, STATE.md 5).
    recorded: Option<Vec<(Who, terse_core::record::Frame)>>,
    /// Completions and images as a windowed host gives them
    /// ([`Self::realtime`]), and the machines whose image is being fetched.
    realtime: bool,
    fetching: Vec<u32>,
}

impl HeadlessSystem {
    pub fn new(content: Option<ContentService>, script: Script) -> Self {
        HeadlessSystem {
            content,
            script,
            machines: HashMap::new(),
            wanted: Vec::new(),
            images: Vec::new(),
            restores: Vec::new(),
            layouter: Box::new(|_| Box::new(terse_core::page::ReferenceLayouter::default())),
            recorded: None,
            realtime: false,
            fetching: Vec::new(),
        }
    }

    /// Finishes requests and image fetches as a windowed host does
    /// (`terse-host`'s frame loop, [`Headless::realtime`]): each frame gets
    /// what finished before it began, and nothing waits.
    pub fn realtime(mut self) -> Self {
        self.realtime = true;
        self
    }

    /// Keeps every input each machine's frame is given, for
    /// [`Self::take_recorded`].
    pub fn record(&mut self) {
        self.recorded.get_or_insert_with(Vec::new);
    }

    /// The inputs of the machine frames run since the last call, in the
    /// order they ran (empty unless [`Self::record`]).
    pub fn take_recorded(&mut self) -> Vec<(Who, terse_core::record::Frame)> {
        self.recorded.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Lays each machine's page out with a layouter from `make` (given the
    /// machine's serial, 0 for the shell).
    pub fn with_layouter(mut self, make: Box<dyn Fn(u32) -> Box<dyn Layouter + Send>>) -> Self {
        self.layouter = make;
        self
    }

    pub fn content(&self) -> Option<&ContentService> {
        self.content.as_ref()
    }

    /// The layouter of machine `serial`'s page (0: the shell), once it has
    /// run a frame.
    pub fn page_layouter(&self, serial: u32) -> Option<&dyn Layouter> {
        self.machines.get(&serial).map(|m| m.page.layouter())
    }

    /// Runs the system's next frame with the script's input for it, then
    /// finishes every request and image fetch it made.
    pub fn frame(&mut self, system: &mut System) -> SystemFrame {
        if self.realtime
            && let Some(content) = &mut self.content
        {
            for (tag, c) in content.take_finished_tagged() {
                if c.handle == 0 && self.fetching.contains(&tag) {
                    self.fetching.retain(|&s| s != tag);
                    self.images.push((tag, image_from(c.outcome)));
                } else if let Some(m) = self.machines.get_mut(&tag) {
                    m.completions.push(c);
                }
            }
            let (saved, restored) = content.take_snapshot_results();
            for (slot, stored) in saved {
                system.snapshot_done(slot, stored);
            }
            self.restores.extend(restored);
        }
        for (serial, image) in self.images.drain(..) {
            system.deliver_image(serial, image);
        }
        for (serial, restored) in self.restores.drain(..) {
            system.deliver_snapshot(serial, restored);
        }
        let frame = system.shell().frame();
        let events = self.script.events(frame).to_vec();
        let controllers = self.script.controllers(frame);
        let result = system.frame(&events, controllers, self);
        let wanted = std::mem::take(&mut self.wanted);
        match &mut self.content {
            None => self
                .images
                .extend(wanted.into_iter().map(|(serial, _)| (serial, Err(StartFailure::not_found("no content device"))))),
            Some(content) => {
                for (serial, want) in &wanted {
                    match want {
                        Wanted::Image(id) => content.fetch(*serial, *id),
                        Wanted::Program(program, builtins) => content.fetch_program(*serial, program, builtins),
                    }
                }
                if self.realtime {
                    self.fetching.extend(wanted.iter().map(|w| w.0));
                    if content.feeds_updated() {
                        for (&serial, m) in self.machines.iter_mut() {
                            if system.machine_by_serial(serial).is_some_and(|(_, m)| m.content().is_some()) {
                                m.feeds_updated = true;
                            }
                        }
                    }
                    return result;
                }
                for (tag, c) in content.wait_finished_tagged() {
                    if c.handle == 0 && wanted.iter().any(|w| w.0 == tag) {
                        self.images.push((tag, image_from(c.outcome)));
                    } else if let Some(m) = self.machines.get_mut(&tag) {
                        m.completions.push(c);
                    }
                }
                // SPEC §machines.snapshots: snapshots stored are reported, and restored
                // machines delivered, at the start of the next frame.
                let (saved, restored) = content.take_snapshot_results();
                for (slot, stored) in saved {
                    system.snapshot_done(slot, stored);
                }
                self.restores.extend(restored);
                if content.feeds_updated() {
                    for (&serial, m) in self.machines.iter_mut() {
                        if system.machine_by_serial(serial).is_some_and(|(_, m)| m.content().is_some()) {
                            m.feeds_updated = true;
                        }
                    }
                }
            }
        }
        result
    }
}

/// The image a fetch delivered (SPEC §machines.calls: a missing blob, or one that is
/// not an image this host can run, fails the start, with its reason).
pub fn image_from(outcome: Outcome) -> Result<terse_core::Image, StartFailure> {
    match outcome {
        Outcome::Got { data, .. } => terse_core::load_image(&data),
        Outcome::NotFound => Err(StartFailure::not_found("image not found")),
        _ => Err(StartFailure::not_found("image fetch failed")),
    }
}

impl Driver for HeadlessSystem {
    fn frame(&mut self, who: Who, machine: &mut Machine, events: Vec<Event>, controllers: [Controller; CONTROLLERS]) -> FrameResult {
        let layouter = &self.layouter;
        let m = self.machines.entry(who.serial).or_insert_with(|| HeadlessMachine {
            page: PageInput::new(layouter(who.serial)),
            completions: Vec::new(),
            feeds_updated: false,
        });
        let mut events = m.page.events(machine.page(), &events);
        if std::mem::take(&mut m.feeds_updated) {
            events.push(Event::feeds_updated());
        }
        let completions = std::mem::take(&mut m.completions);
        let unix_time = if self.realtime { now() } else { 0 };
        let input = FrameInput { events: &events, completions: &completions, controllers, unix_time, ..Default::default() };
        let frame = machine.frame();
        let result = machine.run_frame(&input);
        if let Some(recorded) = &mut self.recorded {
            recorded.push((who, terse_core::record::Frame::of(frame, &input, &result)));
        }
        m.page.after_frame(machine.page());
        if let Some(content) = &mut self.content {
            content.submit_machine(who.serial, machine);
        }
        result
    }

    fn want_image(&mut self, serial: u32, image: [u8; 32]) {
        self.wanted.push((serial, Wanted::Image(image)));
    }

    fn want_program(&mut self, serial: u32, program: &ProgramId, builtins: &[[u8; 32]]) {
        self.wanted.push((serial, Wanted::Program(program.clone(), builtins.to_vec())));
    }

    fn removed(&mut self, serial: u32) {
        self.machines.remove(&serial);
        self.fetching.retain(|&s| s != serial);
        self.images.retain(|i| i.0 != serial);
        self.restores.retain(|i| i.0 != serial);
        if let Some(content) = &mut self.content {
            content.cancel(serial);
        }
    }

    fn save_snapshot(&mut self, shell: &Machine, slot: u8, snapshot: Snapshot) -> Option<bool> {
        let Some(content) = self.content.as_mut() else { return Some(false) };
        content.save_snapshot(Owner::of(shell), slot, &snapshot);
        None
    }

    fn want_snapshot(&mut self, serial: u32, shell: &Machine, slot: u8) -> Option<Result<(terse_core::Image, Snapshot), StartFailure>> {
        let Some(content) = self.content.as_mut() else {
            return Some(Err(StartFailure { reason: failure::SNAPSHOT, image: None, message: "no content device".into() }));
        };
        content.restore_snapshot(serial, Owner::of(shell), slot);
        None
    }

    fn delete_snapshot(&mut self, shell: &Machine, slot: u8) {
        if let Some(content) = self.content.as_mut() {
            content.delete_snapshot(Owner::of(shell), slot);
        }
    }
}

fn completion(job: Job, done: Done) -> Completion {
    let (handle, outcome) = match (job, done) {
        (Job::Get { handle, offset, length }, Done::Got(blob)) => {
            let start = (offset as usize).min(blob.len());
            let end = (start + length as usize).min(blob.len());
            (handle, Outcome::Got { size: blob.len() as u32, data: blob[start..end].to_vec() })
        }
        (Job::Put { handle }, Done::Put(id)) => (handle, Outcome::Put { id }),
        (Job::Publish { handle }, Done::Published { seq, .. }) => (handle, Outcome::Published { seq }),
        (Job::Find { handle, .. }, Done::Found(_)) => (handle, Outcome::Failed),
        // NETWORK 4.7: no relay answered and one refused (SPEC §content.feeds).
        (Job::Find { handle, .. }, Done::Refused) => (handle, Outcome::Refused),
        (Job::Get { handle, .. } | Job::Put { handle } | Job::Publish { handle }, Done::NotFound) => {
            (handle, Outcome::NotFound)
        }
        (Job::Get { handle, .. } | Job::Put { handle } | Job::Publish { handle } | Job::Find { handle, .. }, _) => {
            (handle, Outcome::Failed)
        }
        (Job::Fetch, Done::Got(blob)) => (0, Outcome::Got { size: blob.len() as u32, data: blob }),
        (Job::Fetch, Done::NotFound) => (0, Outcome::NotFound),
        (Job::Fetch, _) => (0, Outcome::Failed),
        (Job::FeedPublish | Job::Lookup(_) | Job::Restore(_), _) => {
            unreachable!("publishes are feed events; lookups go on to a fetch; restores are reported apart")
        }
    };
    Completion { handle, outcome }
}

fn now() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs().min(u32::MAX as u64) as u32)
}

/// What the worker does with a subscribe, unsubscribe or publish request.
enum Step {
    Complete(Outcome),
    Ask(Asked),
    /// Carried out: a publish in progress.
    Started,
}

/// SPEC §content.feeds: requests that change nothing complete at once without asking
/// (subscribing to a feed already subscribed, unsubscribing from one that
/// is not); an invalid feed key (subscribe, unsubscribe, set nickname) is
/// refused, before anything else is looked at, and a blob not in the store is
/// not found, also without asking. The rest are asked about, unless the
/// policy (or the user's answer, `consented`) decides.
fn feed_step(node: &mut Node, consent: Consent, consented: bool, request: &Request, token: Token) -> Step {
    let decided = if consented { Consent::Allow } else { consent };
    let asked = match &request.kind {
        RequestKind::Subscribe { feed, .. } | RequestKind::Unsubscribe { feed } if !valid_feed_key(feed) => {
            return Step::Complete(Outcome::Refused);
        }
        // Already subscribed: nothing changes, hints included.
        RequestKind::Subscribe { feed, .. } if node.subscriptions().contains(feed) => return Step::Complete(Outcome::Done),
        RequestKind::Unsubscribe { feed } if !node.subscriptions().contains(feed) => {
            return Step::Complete(Outcome::Done);
        }
        RequestKind::Subscribe { feed, hints } => Asked::Subscribe { feed: *feed, hints: String::from_utf8_lossy(hints).into_owned() },
        RequestKind::Unsubscribe { feed } => Asked::Unsubscribe(*feed),
        RequestKind::SetNickname { feed, .. } if !valid_feed_key(feed) => return Step::Complete(Outcome::Refused),
        RequestKind::SetNickname { feed, name } if node.nicknames().get(feed).unwrap_or("").as_bytes() == &name[..] => {
            return Step::Complete(Outcome::Done);
        }
        RequestKind::SetProfile { name, .. } | RequestKind::SetNickname { name, .. }
            if std::str::from_utf8(name).is_err() =>
        {
            return Step::Complete(Outcome::Refused);
        }
        RequestKind::SetProfile { name, listed } => {
            Asked::SetProfile { name: String::from_utf8_lossy(name).into_owned(), listed: *listed }
        }
        RequestKind::SetNickname { feed, name } => {
            Asked::SetNickname { feed: *feed, name: String::from_utf8_lossy(name).into_owned() }
        }
        RequestKind::Publish { blob, title, kind } => match node.store().read(blob) {
            // SPEC §content.feeds: a document must start with a document header.
            Ok(Some(bytes)) if *kind == terse_core::content::ENTRY_DOCUMENT && terse_core::content::document_format(&bytes).is_none() => {
                return Step::Complete(Outcome::BadRequest);
            }
            Ok(Some(bytes)) => Asked::Publish {
                title: String::from_utf8_lossy(title).into_owned(),
                size: bytes.len() as u32,
                document: *kind == terse_core::content::ENTRY_DOCUMENT,
            },
            Ok(None) => return Step::Complete(Outcome::NotFound),
            Err(_) => return Step::Complete(Outcome::Failed),
        },
        _ => unreachable!("only subscribe, unsubscribe, publish, set profile and set nickname need consent"),
    };
    match decided {
        Consent::Deny => Step::Complete(Outcome::Refused),
        Consent::Ask => Step::Ask(asked),
        Consent::Allow => match &request.kind {
            // A subscription list that cannot be saved fails.
            RequestKind::Subscribe { feed, hints } => {
                // The device checked the hints (SPEC §content.feeds).
                let hints = std::str::from_utf8(hints).ok().filter(|h| !h.is_empty());
                Step::Complete(if node.subscribe_with_hints(*feed, hints).is_ok() { Outcome::Done } else { Outcome::Failed })
            }
            RequestKind::Unsubscribe { feed } => {
                Step::Complete(if node.unsubscribe(feed).is_ok() { Outcome::Done } else { Outcome::Failed })
            }
            RequestKind::Publish { blob, title, kind } => {
                node.start_publish_blob(token, *blob, &String::from_utf8_lossy(title), *kind, now());
                Step::Started
            }
            RequestKind::SetProfile { name, listed } => {
                node.start_publish_profile(token, &String::from_utf8_lossy(name), *listed, now());
                Step::Started
            }
            RequestKind::SetNickname { feed, name } => {
                let name = String::from_utf8_lossy(name);
                Step::Complete(if node.set_nickname(*feed, Some(&name)).is_ok() { Outcome::Done } else { Outcome::Failed })
            }
            _ => unreachable!(),
        },
    }
}

/// The blob ID of the current version of `program` the host holds: for
/// the all-zero key, the first of `builtins` whose image title is the name
/// (SPEC §machines.calls).
fn find_program(node: &Node, program: &ProgramId, builtins: &[[u8; 32]]) -> Option<[u8; 32]> {
    if program.is_builtin() {
        return builtins.iter().copied().find(|id| {
            let Ok(Some(bytes)) = node.store().read(id) else { return false };
            let title = bytes.get(16..32).unwrap_or_default();
            let len = title.iter().position(|&b| b == 0).unwrap_or(title.len());
            title[..len] == program.name[..]
        });
    }
    node.current_program(&program.publisher, &program.name).map(|e| e.blob)
}

/// Carries out a save, load, delete or records request for `owner` (SPEC
/// §content.records). A storage error fails it.
fn records_request(records: &crate::records::Records, owner: &Owner, kind: RequestKind) -> Outcome {
    let result = match kind {
        RequestKind::Save { name, data } => records.save(owner, &name, &data).map(|saved| match saved {
            Saved::Done => Outcome::Done,
            Saved::NoRoom => Outcome::NoRoom,
        }),
        RequestKind::Load { name, offset, length } => records.load(owner, &name).map(|data| match data {
            None => Outcome::NotFound,
            Some(data) => {
                let start = (offset as usize).min(data.len());
                let end = (start + length as usize).min(data.len());
                Outcome::Got { size: data.len() as u32, data: data[start..end].to_vec() }
            }
        }),
        RequestKind::Delete { name } => records.delete(owner, &name).map(|()| Outcome::Done),
        RequestKind::Records { offset, length } => records.list(owner).map(|list| {
            let (total, data) = crate::records::list_records(&list, offset, length);
            Outcome::Records { total, data }
        }),
        _ => unreachable!("only records requests"),
    };
    result.unwrap_or(Outcome::Failed)
}

/// Carries out (or asks about) a subscribe, unsubscribe or publish; false
/// if the service is gone.
#[allow(clippy::too_many_arguments)]
fn feed_request(
    node: &mut Node,
    consent: Consent,
    consented: bool,
    tag: u32,
    request: Request,
    token: Token,
    pending: &mut HashMap<Token, (u32, Job)>,
    finished: &Sender<Finished>,
) -> bool {
    let handle = request.handle;
    match feed_step(node, consent, consented, &request, token) {
        Step::Complete(outcome) => finished.send(Finished::Done(tag, Completion { handle, outcome })).is_ok(),
        Step::Ask(asked) => finished.send(Finished::Ask(Prompt { tag, handle, asked }, request)).is_ok(),
        Step::Started => {
            pending.insert(token, (tag, Job::Publish { handle }));
            true
        }
    }
}

fn worker(mut node: Node, consent: Consent, jobs: Receiver<Work>, finished: Sender<Finished>, events: Sender<FeedEvent>) {
    let mut pending: HashMap<Token, (u32, Job)> = HashMap::new();
    let mut next: Token = 0;
    loop {
        // Block for work only when nothing is waiting on the relay and no
        // feed is followed.
        let wait = if node.busy() || node.connected() || node.connecting() {
            Some(Duration::ZERO)
        } else if node.has_relays() {
            Some(IDLE_WAIT)
        } else if node.subscriptions_in_file() {
            // Another program may change the subscriptions.
            Some(SUBSCRIPTIONS_WAIT)
        } else {
            None
        };
        let work = match wait {
            Some(wait) => match jobs.recv_timeout(wait) {
                Ok(w) => Some(w),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match jobs.recv() {
                Ok(w) => Some(w),
                Err(_) => return,
            },
        };
        next += 1;
        match work {
            Some(Work::Content(tag, Request { handle, kind: RequestKind::Identity }, _)) => {
                // SPEC §content.feeds: with the hints of the host's own address, if any.
                let hints = crate::address::hint_text(&node.own_hints());
                let outcome = match node.identity_key() {
                    Some(key) if hints.is_empty() => Outcome::Identity { key },
                    Some(key) => Outcome::IdentityHints { key, hints: hints.into_bytes() },
                    None => Outcome::NotFound,
                };
                if finished.send(Finished::Done(tag, Completion { handle, outcome })).is_err() {
                    return;
                }
            }
            Some(Work::Listed(listed)) => node.fetch_blobs(listed),
            Some(Work::Fetch(tag, id)) => {
                pending.insert(next, (tag, Job::Fetch));
                node.start_get(next, id);
            }
            Some(Work::Program(tag, program, builtins)) => match find_program(&node, &program, &builtins) {
                Some(id) => {
                    pending.insert(next, (tag, Job::Fetch));
                    node.start_get(next, id);
                }
                // SPEC §machines.calls: a published program the host does not hold is
                // looked up on the relays first.
                None if !program.is_builtin() => {
                    pending.insert(next, (tag, Job::Lookup(program.clone())));
                    node.start_lookup(next, program.publisher, &program.name);
                }
                None => {
                    let c = Completion { handle: 0, outcome: Outcome::NotFound };
                    if finished.send(Finished::Done(tag, c)).is_err() {
                        return;
                    }
                }
            },
            // SPEC §content.records: named records, answered at once from this host's
            // records directory.
            Some(Work::Content(
                tag,
                Request {
                    handle,
                    kind:
                        kind @ (RequestKind::Save { .. }
                        | RequestKind::Load { .. }
                        | RequestKind::Delete { .. }
                        | RequestKind::Records { .. }),
                },
                owner,
            )) => {
                let outcome = match (node.records(), owner) {
                    (Some(records), Some(owner)) => records_request(records, &owner, kind),
                    _ => Outcome::Failed,
                };
                if finished.send(Finished::Done(tag, Completion { handle, outcome })).is_err() {
                    return;
                }
            }
            Some(Work::Content(tag, Request { handle, kind: RequestKind::Programs { offset, length } }, _)) => {
                let (total, data) = node.program_records(offset, length);
                let outcome = Outcome::Records { total, data };
                if finished.send(Finished::Done(tag, Completion { handle, outcome })).is_err() {
                    return;
                }
            }
            Some(Work::Content(
                tag,
                r @ Request {
                    kind:
                        RequestKind::Subscribe { .. }
                        | RequestKind::Unsubscribe { .. }
                        | RequestKind::Publish { .. }
                        | RequestKind::SetProfile { .. }
                        | RequestKind::SetNickname { .. },
                    ..
                },
                _,
            )) => {
                if !feed_request(&mut node, consent, false, tag, r, next, &mut pending, &finished) {
                    return;
                }
            }
            Some(Work::Consented(tag, r)) => {
                if !feed_request(&mut node, consent, true, tag, r, next, &mut pending, &finished) {
                    return;
                }
            }
            Some(Work::Content(tag, Request { handle, kind: RequestKind::Get { id, offset, length } }, _)) => {
                pending.insert(next, (tag, Job::Get { handle, offset, length }));
                node.start_get(next, id);
            }
            Some(Work::Content(tag, Request { handle, kind: RequestKind::Put { data } }, _)) => {
                pending.insert(next, (tag, Job::Put { handle }));
                node.start_put(next, &data);
            }
            // SPEC §content.feeds: answered at once from the node's own state.
            Some(Work::Content(tag, Request { handle, kind: RequestKind::Subscriptions { offset, length } }, _)) => {
                let (total, data) = node.subscription_records(offset, length);
                let outcome = Outcome::Records { total, data };
                if finished.send(Finished::Done(tag, Completion { handle, outcome })).is_err() {
                    return;
                }
            }
            Some(Work::Content(tag, Request { handle, kind: RequestKind::Timeline { offset, length } }, _)) => {
                let (total, data) = node.timeline_records(offset, length);
                let outcome = Outcome::Records { total, data };
                if finished.send(Finished::Done(tag, Completion { handle, outcome })).is_err() {
                    return;
                }
            }
            Some(Work::Content(tag, Request { handle, kind: RequestKind::Names { offset, length } }, _)) => {
                let (total, data) = node.name_records(offset, length);
                let outcome = Outcome::Records { total, data };
                if finished.send(Finished::Done(tag, Completion { handle, outcome })).is_err() {
                    return;
                }
            }
            Some(Work::Content(tag, Request { handle, kind: RequestKind::Find { scope, query, length } }, _)) => {
                pending.insert(next, (tag, Job::Find { handle, length }));
                node.start_find(next, scope, &query);
            }
            Some(Work::Feed(FeedCommand::Publish(draft))) => {
                pending.insert(next, (0, Job::FeedPublish));
                node.start_publish_article(next, &draft);
            }
            // SPEC §machines.snapshots: snapshots, in this host's data directory.
            Some(Work::SaveSnapshot(owner, slot, file)) => {
                let ok = node.snapshots().is_some_and(|s| s.save(&owner, slot, &file).is_ok());
                if finished.send(Finished::Saved(slot, ok)).is_err() {
                    return;
                }
            }
            Some(Work::DeleteSnapshot(owner, slot)) => {
                if let Some(s) = node.snapshots() {
                    let _ = s.delete(&owner, slot);
                }
                if finished.send(Finished::Deleted).is_err() {
                    return;
                }
            }
            Some(Work::Restore(tag, owner, slot)) => {
                let unreadable = |message: String| StartFailure { reason: failure::SNAPSHOT, image: None, message };
                let read = match node.snapshots().map(|s| s.load(&owner, slot)) {
                    None => Err(unreadable("this host keeps no snapshots".into())),
                    Some(Err(e)) => Err(unreadable(format!("snapshot {slot}: {e}"))),
                    Some(Ok(None)) => Err(unreadable(format!("no snapshot in slot {slot}"))),
                    Some(Ok(Some(file))) => Snapshot::decode(&file).map_err(|e| unreadable(format!("snapshot {slot}: {e}"))),
                };
                match read {
                    Ok(snapshot) => {
                        let id = snapshot.image_id();
                        pending.insert(next, (tag, Job::Restore(Box::new(snapshot))));
                        node.start_get(next, id);
                    }
                    Err(f) => {
                        if finished.send(Finished::Restored(tag, Err(f))).is_err() {
                            return;
                        }
                    }
                }
            }
            // A subscription list that cannot be saved still applies to
            // this run.
            Some(Work::Feed(FeedCommand::Subscribe(feed))) => drop(node.subscribe(feed)),
            Some(Work::Feed(FeedCommand::Unsubscribe(feed))) => drop(node.unsubscribe(&feed)),
            None => {}
        }
        // What finished at once (a held blob, from the store) goes back
        // before the relays are polled: polling them waits for each
        // connection's socket a few milliseconds.
        let ready = node.take_ready();
        let finished_now = if ready.is_empty() { node.poll() } else { ready };
        for (token, done) in finished_now {
            let sent = match pending.remove(&token) {
                Some((_, Job::FeedPublish)) => events
                    .send(match done {
                        Done::Published { seq, blob } => FeedEvent::Published { seq, blob },
                        done => FeedEvent::PublishFailed(done),
                    })
                    .is_ok(),
                Some((tag, Job::Find { handle, length })) if matches!(done, Done::Found(_)) => {
                    let Done::Found(found) = done else { unreachable!() };
                    let (total, data) = node.find_records(&found, length);
                    let outcome = Outcome::Records { total, data };
                    finished.send(Finished::Done(tag, Completion { handle, outcome })).is_ok()
                }
                Some((tag, Job::Lookup(program))) => match done {
                    Done::Found(_) if let Some(id) = node.current_program(&program.publisher, &program.name).map(|e| e.blob) => {
                        next += 1;
                        pending.insert(next, (tag, Job::Fetch));
                        node.start_get(next, id);
                        true
                    }
                    _ => finished.send(Finished::Done(tag, Completion { handle: 0, outcome: Outcome::NotFound })).is_ok(),
                },
                // SPEC §machines.snapshots: the snapshot's image, by its ID.
                Some((tag, Job::Restore(snapshot))) => {
                    if let Done::Got(blob) = &done {
                        node.fetch_listed(blob);
                    }
                    let result = match done {
                        Done::Got(blob) => terse_core::load_image(&blob).map(|image| (image, *snapshot)),
                        Done::NotFound => Err(StartFailure::not_found("the snapshot's image was not found")),
                        _ => Err(StartFailure::not_found("the snapshot's image could not be fetched")),
                    };
                    finished.send(Finished::Restored(tag, result)).is_ok()
                }
                Some((tag, job)) => {
                    // SPEC §image.blobs: an image fetched to start it brings its blobs.
                    if let (Job::Fetch, Done::Got(blob)) = (&job, &done) {
                        node.fetch_listed(blob);
                    }
                    finished.send(Finished::Done(tag, completion(job, done))).is_ok()
                }
                None => true,
            };
            if !sent {
                return;
            }
        }
        // A relay's refusal of a put or publish, with its reason (NETWORK
        // 3.1), goes to the log: the program sees only that it failed.
        for (url, d) in node.take_delivery() {
            match d {
                crate::node::Delivery::Refused(why) => eprintln!("terse-host: {url} refused a put or publish: {}", why.describe()),
                crate::node::Delivery::BlobRefused(why) => {
                    eprintln!("terse-host: {url} took an entry but refused its blob: {}", why.describe())
                }
                _ => {}
            }
        }
        for event in node.take_feed_events() {
            if events.send(event).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::blob_id;
    use crate::store::BlobStore;

    #[test]
    fn serves_ranges_from_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path(), None).unwrap();
        let id = store.put(b"0123456789").unwrap();
        let mut service = ContentService::start(Node::new(store, None));
        let get = |handle, offset, length| Request { handle, kind: RequestKind::Get { id, offset, length } };
        service.submit(vec![
            get(1, 2, 3),
            get(2, 8, 100),
            get(3, 10, 1),
            Request { handle: 4, kind: RequestKind::Get { id: [0; 32], offset: 0, length: 1 } },
            Request { handle: 5, kind: RequestKind::Put { data: b"new".to_vec() } },
        ]);
        let mut done = service.wait_finished();
        done.sort_by_key(|c| c.handle);
        let got = |size, data: &[u8]| Outcome::Got { size, data: data.to_vec() };
        let outcomes: Vec<Outcome> = done.into_iter().map(|c| c.outcome).collect();
        assert_eq!(
            outcomes,
            [got(10, b"234"), got(10, b"89"), got(10, b""), Outcome::NotFound, Outcome::Put { id: blob_id(b"new") }]
        );
        assert_eq!(service.outstanding(), 0);
        assert!(service.take_finished().is_empty());
    }

    #[test]
    fn publishes_articles_as_feed_commands() {
        let dir = tempfile::tempdir().unwrap();
        let mut node = Node::new(BlobStore::open(dir.path(), None).unwrap(), None);
        node.set_identity(crate::Identity::from_seed([5; 32]));
        let mut service = ContentService::start(node);
        let draft = Draft { data: b"text".to_vec(), title: "t".into(), reply_to: None, time: 0 };
        service.feed(FeedCommand::Publish(draft.clone()));
        service.feed(FeedCommand::Publish(Draft { title: "x".repeat(121), ..draft }));
        let timeout = std::time::Duration::from_secs(5);
        assert_eq!(service.wait_feed_event(timeout), Some(FeedEvent::Published { seq: 1, blob: blob_id(b"text") }));
        assert_eq!(service.wait_feed_event(timeout), Some(FeedEvent::PublishFailed(Done::Failed)));
        assert!(service.take_feed_events().is_empty());
    }

    #[test]
    fn answers_feed_requests() {
        let dir = tempfile::tempdir().unwrap();
        let mut node = Node::new(BlobStore::open(dir.path(), None).unwrap(), None);
        let author = crate::Identity::from_seed([6; 32]);
        node.set_identity(author.clone());
        for title in ["one", "two"] {
            let draft = Draft { data: title.as_bytes().to_vec(), title: title.into(), reply_to: None, time: 0 };
            assert!(matches!(node.publish_article(&draft), Done::Published { .. }));
        }
        node.set_subscriptions(crate::Subscriptions::in_memory([author.feed_key()]));
        let mut service = ContentService::start(node);
        service.submit(vec![
            Request { handle: 1, kind: RequestKind::Subscriptions { offset: 0, length: 100 } },
            Request { handle: 2, kind: RequestKind::Timeline { offset: 1, length: 1000 } },
        ]);
        let mut done = service.wait_finished();
        done.sort_by_key(|c| c.handle);
        let Outcome::Records { total, data } = &done[0].outcome else { panic!("{done:?}") };
        assert_eq!((*total, &data[..32], &data[32..]), (1, &author.feed_key()[..], &2u32.to_le_bytes()[..]));
        let Outcome::Records { total, data } = &done[1].outcome else { panic!("{done:?}") };
        // Index 1 is the older entry; its article is in the store.
        assert_eq!((*total, data.len(), data[0], data[1]), (2, 2 + 109 + 3, 1 + 109 + 3, 1));
        assert_eq!(&data[2 + 109..], b"one");
        assert!(!service.feeds_updated(), "own publishing is not a new entry of a followed feed");
    }

    fn feed_requests(node: Node, consent: Consent, kinds: Vec<RequestKind>) -> (ContentService, Vec<Completion>) {
        let mut service = ContentService::start_with_consent(node, consent);
        service.submit(kinds.into_iter().enumerate().map(|(i, kind)| Request { handle: i as u8 + 1, kind }).collect());
        let mut done = service.wait_finished();
        done.sort_by_key(|c| c.handle);
        (service, done)
    }

    fn outcomes(done: &[Completion]) -> Vec<Outcome> {
        done.iter().map(|c| c.outcome.clone()).collect()
    }

    /// SPEC §content.feeds: with consent denied, subscribe and publish are refused;
    /// requests that change nothing, unknown blobs and the identity are
    /// answered without asking.
    #[test]
    fn consent_denied() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path(), None).unwrap();
        let article = store.put(b"article").unwrap();
        let mut node = Node::new(store, None);
        let a = crate::Identity::from_seed([1; 32]).feed_key();
        let b = crate::Identity::from_seed([2; 32]).feed_key();
        node.set_subscriptions(crate::Subscriptions::in_memory([a]));
        let (_, done) = feed_requests(
            node,
            Consent::Deny,
            vec![
                RequestKind::Subscribe { feed: b, hints: Vec::new() },
                RequestKind::Subscribe { feed: a, hints: Vec::new() },
                RequestKind::Unsubscribe { feed: b },
                RequestKind::Unsubscribe { feed: a },
                RequestKind::Publish { blob: article, title: b"t".to_vec(), kind: 1 },
                RequestKind::Publish { blob: [9; 32], title: b"t".to_vec(), kind: 1 },
                RequestKind::Identity,
                RequestKind::Subscribe { feed: [0; 32], hints: Vec::new() },
            ],
        );
        assert_eq!(
            outcomes(&done),
            [
                Outcome::Refused,
                Outcome::Done,
                Outcome::Done,
                Outcome::Refused,
                Outcome::Refused,
                Outcome::NotFound,
                Outcome::NotFound,
                Outcome::Refused,
            ]
        );
    }

    #[test]
    fn consent_allowed_subscribes_and_publishes() {
        let dir = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path(), None).unwrap();
        let article = store.put(b"An article.").unwrap();
        let mut node = Node::new(store, None);
        let subs = config.path().join("subscriptions");
        node.set_subscriptions(crate::Subscriptions::load(&subs).unwrap());
        node.set_identity_path(config.path().join("identity.key")).unwrap();
        let b = crate::Identity::from_seed([2; 32]).feed_key();
        let (mut service, done) = feed_requests(
            node,
            Consent::Allow,
            vec![RequestKind::Subscribe { feed: b, hints: Vec::new() }, RequestKind::Publish { blob: article, title: "Tïtle".into(), kind: 1 }],
        );
        assert_eq!(outcomes(&done), [Outcome::Done, Outcome::Published { seq: 1 }]);
        // The identity was created; the subscription saved.
        let me = crate::Identity::load(&config.path().join("identity.key")).unwrap().feed_key();
        assert_eq!(std::fs::read_to_string(&subs).unwrap().trim(), crate::to_hex(&b));
        service.submit(vec![Request { handle: 1, kind: RequestKind::Identity }]);
        assert_eq!(outcomes(&service.wait_finished()), [Outcome::Identity { key: me }]);
        let held = crate::FeedStore::beside_blobs(dir.path()).entries(&me).unwrap();
        assert_eq!((held.len(), held[0].entry.title.as_str(), held[0].entry.blob), (1, "Tïtle", article));
    }

    /// SPEC §content.feeds: subscribe and unsubscribe with an invalid feed key complete
    /// with 05h (refused) without asking, even where the user would allow
    /// them and where unsubscribing would otherwise change nothing.
    #[test]
    fn invalid_feed_keys_are_refused_without_asking() {
        let dir = tempfile::tempdir().unwrap();
        let bad = [0; 32];
        assert!(!valid_feed_key(&bad));
        for consent in [Consent::Allow, Consent::Deny, Consent::Ask] {
            let node = Node::new(BlobStore::open(dir.path(), None).unwrap(), None);
            let (service, done) =
                feed_requests(node, consent, vec![RequestKind::Unsubscribe { feed: bad }, RequestKind::Subscribe { feed: bad, hints: Vec::new() }]);
            assert_eq!(outcomes(&done), [Outcome::Refused, Outcome::Refused], "{consent:?}");
            assert!(service.prompt().is_none(), "nothing is asked");
        }
    }

    /// SPEC §content.feeds ops 09h–0Ch: names records; a find with no relay fails;
    /// set profile and set nickname with consent denied, allowed and asked.
    #[test]
    fn names_find_profile_and_nickname_requests() {
        let dir = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let make = |consent| {
            let mut node = Node::new(BlobStore::open(dir.path(), None).unwrap(), None);
            node.set_identity_path(config.path().join("identity.key")).unwrap();
            node.set_nicknames(crate::profile::Nicknames::load_from_config(config.path()).unwrap());
            node.set_subscriptions(crate::Subscriptions::load(config.path().join("subscriptions")).unwrap());
            ContentService::start_with_consent(node, consent)
        };
        let b = crate::Identity::from_seed([2; 32]).feed_key();
        let kinds = || {
            vec![
                RequestKind::SetProfile { name: b"Ana".to_vec(), listed: true },
                RequestKind::SetNickname { feed: b, name: b"Bee".to_vec() },
                RequestKind::SetNickname { feed: [0; 32], name: b"Zero".to_vec() },
                RequestKind::SetNickname { feed: b, name: Vec::new() },
                RequestKind::Find { scope: 1, query: b"ana".to_vec(), length: 100 },
            ]
        };
        let submit = |service: &mut ContentService, kinds: Vec<RequestKind>| {
            service.submit(kinds.into_iter().enumerate().map(|(i, kind)| Request { handle: i as u8 + 1, kind }).collect());
            let mut done = service.wait_finished();
            done.sort_by_key(|c| c.handle);
            outcomes(&done)
        };
        // Denied: refused; an invalid key refused; removing a nickname
        // there is not done at once; no relay to find on.
        let mut service = make(Consent::Deny);
        assert_eq!(
            submit(&mut service, kinds()),
            [Outcome::Refused, Outcome::Refused, Outcome::Refused, Outcome::Done, Outcome::Failed]
        );
        assert!(!config.path().join("identity.key").exists());
        drop(service);
        // Allowed: the profile is published (the identity created), the
        // nickname saved.
        let mut service = make(Consent::Allow);
        assert_eq!(
            submit(&mut service, kinds()[..2].to_vec()),
            [Outcome::Published { seq: 1 }, Outcome::Done]
        );
        let me = crate::Identity::load(&config.path().join("identity.key")).unwrap().feed_key();
        let saved = crate::profile::Nicknames::load_from_config(config.path()).unwrap();
        assert_eq!(saved.get(&b), Some("Bee"));
        // Names: subscribe to both, read the records.
        drop(service);
        let mut subs = crate::Subscriptions::load(config.path().join("subscriptions")).unwrap();
        subs.add(me).unwrap();
        subs.add(b).unwrap();
        let mut service = make(Consent::Ask);
        let names = submit(&mut service, vec![RequestKind::Names { offset: 0, length: 1000 }]);
        let rec = |k: &[u8; 32], flags: u8, name: &str| [&[33 + name.len() as u8, flags][..], k, name.as_bytes()].concat();
        let mut want = [(me, rec(&me, 2, "Ana")), (b, rec(&b, 1, "Bee"))];
        want.sort_by_key(|w| w.0);
        assert_eq!(names, [Outcome::Records { total: 2, data: [want[0].1.clone(), want[1].1.clone()].concat() }]);
        // Asked: the prompts say what is asked; the same nickname again
        // completes at once.
        service.submit(vec![
            Request { handle: 1, kind: RequestKind::SetProfile { name: b"Ana B".to_vec(), listed: false } },
            Request { handle: 2, kind: RequestKind::SetNickname { feed: b, name: b"Bee".to_vec() } },
            Request { handle: 3, kind: RequestKind::SetNickname { feed: b, name: Vec::new() } },
        ]);
        let done = service.wait_finished();
        assert_eq!(done, [Completion { handle: 2, outcome: Outcome::Done }]);
        assert_eq!(service.prompt().unwrap().asked, Asked::SetProfile { name: "Ana B".into(), listed: false });
        service.answer(true);
        assert_eq!(service.prompt().unwrap().asked, Asked::SetNickname { feed: b, name: String::new() });
        service.answer(false);
        let mut done = service.wait_finished();
        done.sort_by_key(|c| c.handle);
        assert_eq!(outcomes(&done), [Outcome::Published { seq: 2 }, Outcome::Refused]);
    }

    fn owner_service(dir: &std::path::Path, data: &std::path::Path) -> ContentService {
        let mut node = Node::new(BlobStore::open(dir, None).unwrap(), None);
        node.set_records(crate::Records::in_data_dir(data));
        ContentService::start(node)
    }

    /// Runs `kinds` as the requests of a machine with `identity` (else
    /// with no identity) of image `image`; the outcomes in handle order.
    fn as_machine(service: &mut ContentService, image: &terse_core::Image, identity: Option<ProgramId>, kinds: Vec<RequestKind>) -> Vec<Outcome> {
        let owner = match identity {
            Some(id) => Owner::Program { publisher: id.publisher, name: id.name },
            None => Owner::Image(image.id),
        };
        for (i, kind) in kinds.into_iter().enumerate() {
            service.outstanding += 1;
            let _ = service.jobs.send(Work::Content(1, Request { handle: i as u8 + 1, kind }, Some(owner.clone())));
        }
        let mut done = service.wait_finished();
        done.sort_by_key(|c| c.handle);
        outcomes(&done)
    }

    fn image(title: &str, rom: u8) -> terse_core::Image {
        terse_core::Image::parse(&terse_core::Image::build(title, 1, &[], &[rom, 0, 0, 0])).unwrap()
    }

    /// SPEC §content.records: save, load (ranges), replace, delete and the records list,
    /// for the owner of the requests; owners do not see each other's
    /// records; a new version of a program (same feed and name) reads the
    /// old version's; records outlive the service; a machine started from
    /// a file has the records of its exact image.
    #[test]
    fn named_records() {
        let (dir, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut service = owner_service(dir.path(), data.path());
        let sparks = |publisher: u8| Some(ProgramId { publisher: [publisher; 32], name: b"sparks".to_vec() });
        let (v1, v2) = (image("sparks", 1), image("sparks", 2));
        let save = |name: &str, data: &[u8]| RequestKind::Save { name: name.into(), data: data.to_vec() };
        let load = |name: &str, offset, length| RequestKind::Load { name: name.into(), offset, length };
        let list = |offset, length| RequestKind::Records { offset, length };
        assert_eq!(
            as_machine(&mut service, &v1, sparks(3), vec![
                load("high-score", 0, 4),
                save("high-score", b"\x2a\0\0\0"),
                save("notes", b"0123456789"),
                load("notes", 3, 4),
                load("notes", 8, 100),
                load("notes", 10, 100),
                list(0, 100),
                list(1, 36),
            ]),
            [
                Outcome::NotFound,
                Outcome::Done,
                Outcome::Done,
                Outcome::Got { size: 10, data: b"3456".to_vec() },
                Outcome::Got { size: 10, data: b"89".to_vec() },
                Outcome::Got { size: 10, data: Vec::new() },
                Outcome::Records { total: 2, data: [&b"high-score"[..], &[0; 22], &4u32.to_le_bytes(), b"notes", &[0; 27], &10u32.to_le_bytes()].concat() },
                Outcome::Records { total: 2, data: [&b"notes"[..], &[0; 27], &10u32.to_le_bytes()].concat() },
            ]
        );
        // The next version of the program reads them; replace and delete.
        assert_eq!(
            as_machine(&mut service, &v2, sparks(3), vec![
                load("high-score", 0, 16),
                save("high-score", b"\x2b\0\0\0"),
                RequestKind::Delete { name: b"notes".to_vec() },
                RequestKind::Delete { name: b"never".to_vec() },
                list(0, 100),
            ]),
            [
                Outcome::Got { size: 4, data: b"\x2a\0\0\0".to_vec() },
                Outcome::Done,
                Outcome::Done,
                Outcome::Done,
                Outcome::Records { total: 1, data: [&b"high-score"[..], &[0; 22], &4u32.to_le_bytes()].concat() },
            ]
        );
        // Another publisher's program of the same name, the built-in of
        // that title, and the image run from a file see none of them.
        for (img, id) in [(&v1, sparks(4)), (&v1, sparks(0)), (&v1, None), (&v2, None)] {
            assert_eq!(as_machine(&mut service, img, id.clone(), vec![load("high-score", 0, 4), list(0, 100)]), [
                Outcome::NotFound,
                Outcome::Records { total: 0, data: Vec::new() }
            ], "{id:?}");
        }
        // A file-run image has records of its own, by its exact ID.
        assert_eq!(as_machine(&mut service, &v1, None, vec![save("k", b"v1")]), [Outcome::Done]);
        assert_eq!(as_machine(&mut service, &v2, None, vec![load("k", 0, 4)]), [Outcome::NotFound]);
        assert_eq!(as_machine(&mut service, &v1, None, vec![load("k", 0, 4)]), [Outcome::Got { size: 2, data: b"v1".to_vec() }]);
        // Kept across services (runs).
        drop(service);
        let mut service = owner_service(dir.path(), data.path());
        assert_eq!(as_machine(&mut service, &v1, sparks(3), vec![load("high-score", 0, 4)]), [Outcome::Got {
            size: 4,
            data: b"\x2b\0\0\0".to_vec()
        }]);
        // The limits: no room, and nothing changed.
        let big = vec![1u8; 65_536];
        let mut kinds: Vec<RequestKind> = (0..8).map(|i| save(&format!("big{i}"), &big)).collect();
        kinds.truncate(8);
        assert!(as_machine(&mut service, &v1, sparks(5), kinds).iter().all(|o| *o == Outcome::Done));
        let kinds: Vec<RequestKind> = (8..16).map(|i| save(&format!("big{i}"), &big)).collect();
        assert!(as_machine(&mut service, &v1, sparks(5), kinds).iter().all(|o| *o == Outcome::Done));
        assert_eq!(as_machine(&mut service, &v1, sparks(5), vec![save("one", b"x"), load("one", 0, 1)]), [Outcome::NoRoom, Outcome::NotFound]);
        // A host that keeps no records fails them; so do requests whose
        // owner is not known.
        let mut plain = ContentService::start(Node::new(BlobStore::open(dir.path(), None).unwrap(), None));
        assert_eq!(as_machine(&mut plain, &v1, None, vec![save("k", b"")]), [Outcome::Failed]);
        plain.submit(vec![Request { handle: 1, kind: save("k", b"") }]);
        assert_eq!(outcomes(&plain.wait_finished()), [Outcome::Failed]);
    }

    /// SPEC §machines.calls `start_program`: the current version the host holds, a
    /// built-in by title for the zero key, not found otherwise; op 0Dh.
    #[test]
    fn programs_and_their_images() {
        let dir = tempfile::tempdir().unwrap();
        let mut node = Node::new(BlobStore::open(dir.path(), None).unwrap(), None);
        let author = crate::Identity::from_seed([6; 32]);
        node.set_identity(author.clone());
        let (v1, v2) = (image("tool", 1), image("tool", 2));
        let bytes = |i: &terse_core::Image| terse_core::Image::build(&i.title_lossy(), 1, &[], &i.rom);
        assert!(matches!(node.publish_program(&bytes(&v1), "tool", 1), Done::Published { seq: 1, .. }));
        assert!(matches!(node.publish_program(&bytes(&v2), "tool", 2), Done::Published { seq: 2, .. }));
        assert!(matches!(node.publish_program(b"not an image", "junk", 3), Done::Published { seq: 3, .. }));
        node.set_subscriptions(crate::Subscriptions::in_memory([author.feed_key()]));
        let hello = node.store().put(crate::builtin::PROGRAMS[3]).unwrap();
        let mut service = ContentService::start(node);
        let key = author.feed_key();
        let fetch = |service: &mut ContentService, publisher: [u8; 32], name: &str, builtins: &[[u8; 32]]| {
            service.fetch_program(9, &ProgramId { publisher, name: name.into() }, builtins);
            let done = service.wait_finished_tagged();
            assert_eq!(done.len(), 1);
            assert_eq!((done[0].0, done[0].1.handle), (9, 0));
            image_from(done[0].1.outcome.clone())
        };
        assert_eq!(fetch(&mut service, key, "tool", &[]).unwrap().id, v2.id, "the current version");
        assert_eq!(fetch(&mut service, key, "nope", &[]).unwrap_err().reason, terse_core::machines::failure::NOT_FOUND);
        let junk = fetch(&mut service, key, "junk", &[]).unwrap_err();
        assert_eq!((junk.reason, junk.image), (terse_core::machines::failure::INVALID, Some(crate::blob_id(b"not an image"))));
        assert_eq!(fetch(&mut service, [0; 32], "hello", &[[1; 32], hello]).unwrap().id, hello);
        assert!(fetch(&mut service, [0; 32], "hello", &[]).is_err(), "not among the built-ins");
        assert!(fetch(&mut service, [0; 32], "tool", &[hello]).is_err());
        // Op 0Dh: the current versions, by name.
        service.submit(vec![Request { handle: 1, kind: RequestKind::Programs { offset: 0, length: 1000 } }]);
        let Outcome::Records { total, data } = &service.wait_finished()[0].outcome else { panic!() };
        assert_eq!(*total, 2);
        assert_eq!((data[0] as usize, data[1], &data[2 + 32..2 + 36]), (1 + 109 + 4, 1, &3u32.to_le_bytes()[..]));
        assert_eq!(&data[2 + 109..2 + 113], b"junk");
        assert_eq!(&data[2 + 113 + 2 + 32..2 + 113 + 2 + 36], &2u32.to_le_bytes(), "tool, version 2");
    }

    /// SPEC §content.feeds: a document publish (kind 4) needs a valid document
    /// header (else 04h, without asking); the prompt says it is a
    /// document; the entry is kind 4.
    #[test]
    fn publishing_documents() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path(), None).unwrap();
        let doc = store.put(&[&b"TDOC\0"[..], &[0; 32], &[6], b"sketch", b"lines"].concat()).unwrap();
        let text = store.put(b"just text").unwrap();
        let mut node = Node::new(store, None);
        let me = crate::Identity::from_seed([3; 32]);
        node.set_identity(me.clone());
        let publish = |blob, kind| RequestKind::Publish { blob, title: b"Sketch 1".to_vec(), kind };
        let (mut service, done) = feed_requests(node, Consent::Ask, vec![publish(text, 4), publish(doc, 4)]);
        assert_eq!(done, [Completion { handle: 1, outcome: Outcome::BadRequest }]);
        assert_eq!(service.prompt().unwrap().asked, Asked::Publish { title: "Sketch 1".into(), size: 49, document: true });
        service.answer(true);
        assert_eq!(outcomes(&service.wait_finished()), [Outcome::Published { seq: 1 }]);
        let held = crate::FeedStore::beside_blobs(dir.path()).entries(&me.feed_key()).unwrap();
        assert_eq!((held[0].entry.kind, held[0].entry.blob), (crate::entry::KIND_DOCUMENT, doc));
    }

    /// Asked: one prompt at a time, in handle order; a yes carries the
    /// request out, a no refuses it.
    #[test]
    fn consent_asked() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path(), None).unwrap();
        let article = store.put(b"12345").unwrap();
        let mut node = Node::new(store, None);
        node.set_identity(crate::Identity::from_seed([3; 32]));
        let b = crate::Identity::from_seed([2; 32]).feed_key();
        let (mut service, done) = feed_requests(
            node,
            Consent::Ask,
            vec![
                RequestKind::Subscribe { feed: b, hints: Vec::new() },
                RequestKind::Publish { blob: article, title: b"T".to_vec(), kind: 1 },
                RequestKind::Identity,
            ],
        );
        assert_eq!(done.len(), 1, "only the identity is answered: {done:?}");
        assert_eq!(service.prompts_waiting(), 2);
        assert_eq!(service.prompt(), Some(&Prompt { tag: 0, handle: 1, asked: Asked::Subscribe { feed: b, hints: String::new() } }));
        service.answer(false);
        assert_eq!(service.prompt(), Some(&Prompt { tag: 0, handle: 2, asked: Asked::Publish { title: "T".into(), size: 5, document: false } }));
        service.answer(true);
        assert_eq!(service.prompt(), None);
        let mut done = service.wait_finished();
        done.sort_by_key(|c| c.handle);
        assert_eq!(
            done,
            [
                Completion { handle: 1, outcome: Outcome::Refused },
                Completion { handle: 2, outcome: Outcome::Published { seq: 1 } }
            ]
        );
        assert_eq!(service.outstanding(), 0);
    }

    /// The reading profile (`terse run --profile reading`): get,
    /// subscriptions, timeline, names, identity, subscribe and unsubscribe
    /// are carried out; every other request completes with status Refused
    /// at once, without asking (even with consent allowed) and without
    /// touching the node.
    #[test]
    fn reading_profile_refuses_optional_operations() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path(), None).unwrap();
        let held = store.put(b"held").unwrap();
        let mut node = Node::new(store, None);
        node.set_identity(crate::Identity::from_seed([3; 32]));
        node.set_records(crate::Records::in_data_dir(&dir.path().join("data")));
        let a = crate::Identity::from_seed([1; 32]).feed_key();
        let mut service = ContentService::start_with_consent(node, Consent::Allow);
        service.set_reading_profile(true);
        let kinds = vec![
            RequestKind::Get { id: held, offset: 0, length: 10 },
            RequestKind::Subscriptions { offset: 0, length: 100 },
            RequestKind::Timeline { offset: 0, length: 100 },
            RequestKind::Names { offset: 0, length: 100 },
            RequestKind::Identity,
            RequestKind::Subscribe { feed: a, hints: Vec::new() },
            RequestKind::Unsubscribe { feed: a },
            RequestKind::Put { data: b"new".to_vec() },
            RequestKind::Publish { blob: held, title: b"t".to_vec(), kind: 1 },
            RequestKind::Find { scope: 1, query: b"x".to_vec(), length: 100 },
            RequestKind::SetProfile { name: b"n".to_vec(), listed: true },
            RequestKind::SetNickname { feed: a, name: b"n".to_vec() },
            RequestKind::Programs { offset: 0, length: 100 },
            RequestKind::Save { name: b"r".to_vec(), data: b"d".to_vec() },
            RequestKind::Load { name: b"r".to_vec(), offset: 0, length: 10 },
            RequestKind::Delete { name: b"r".to_vec() },
            RequestKind::Records { offset: 0, length: 100 },
        ];
        assert_eq!(kinds.iter().filter(|k| in_reading_profile(k)).count(), 7);
        service.submit(kinds.into_iter().enumerate().map(|(i, kind)| Request { handle: i as u8 + 1, kind }).collect());
        let mut done = service.wait_finished();
        done.sort_by_key(|c| c.handle);
        let outcomes = outcomes(&done);
        assert_eq!(outcomes.len(), 17);
        assert_eq!(outcomes[0], Outcome::Got { size: 4, data: b"held".to_vec() });
        assert!(matches!(outcomes[1], Outcome::Records { total: 0, .. }));
        assert!(matches!(outcomes[4], Outcome::Identity { .. }));
        assert_eq!(outcomes[5..7], [Outcome::Done, Outcome::Done]);
        assert!(outcomes[7..].iter().all(|o| *o == Outcome::Refused), "{outcomes:?}");
        assert!(!dir.path().join(crate::to_hex(&crate::blob_id(b"new"))).exists(), "the put never reached the store");
        assert_eq!(service.outstanding(), 0);
    }
}
