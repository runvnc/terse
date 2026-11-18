//! Carries out content requests (SPEC 9) and follows feeds (NETWORK 4.4) on a
//! background thread, so the frame loop only ever submits requests and
//! collects completions and feed events without blocking. Blocking waits are
//! for headless and command-line use.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use terse_core::page::{Layouter, PageInput};
use terse_core::{Completion, Event, FrameInput, FrameResult, Machine, Outcome, Request, RequestKind, Script};

use crate::identity::{FeedKey, valid_feed_key};
use crate::node::{Done, Draft, FeedEvent, Node, Token};

/// How long the worker sleeps between reconnection checks while following
/// feeds without a connection.
const IDLE_WAIT: Duration = Duration::from_millis(20);

/// Feed operations for the worker's node.
#[derive(Clone, Debug)]
pub enum FeedCommand {
    Subscribe(FeedKey),
    Unsubscribe(FeedKey),
    /// Reported as `FeedEvent::Published` or `FeedEvent::PublishFailed`.
    Publish(Draft),
}

enum Work {
    Content(Request),
    /// A request the user has allowed (SPEC 9.5).
    Consented(Request),
    Feed(FeedCommand),
}

/// How the host answers the requests that need the user's consent
/// (subscribe, unsubscribe, publish; SPEC 9.5).
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
    Subscribe(FeedKey),
    Unsubscribe(FeedKey),
    /// Publish an article (`size` bytes) with this title.
    Publish { title: String, size: u32 },
    /// Publish a profile with this display name; `listed`: findable by
    /// search.
    SetProfile { name: String, listed: bool },
    /// Give a feed a nickname (empty: remove it).
    SetNickname { feed: FeedKey, name: String },
}

/// A request waiting for the user's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub handle: u8,
    pub asked: Asked,
}

enum Finished {
    Done(Completion),
    Ask(Prompt, Request),
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
    refused: Vec<Completion>,
    /// The node has a relay and subscriptions.
    following: bool,
}

/// What the worker needs to turn a `Done` into a completion.
enum Job {
    Get { handle: u8, offset: u32, length: u16 },
    Put { handle: u8 },
    /// A program's publish or set profile (SPEC 9.5).
    Publish { handle: u8 },
    /// A program's find, with its buffer size.
    Find { handle: u8, length: u16 },
    /// `FeedCommand::Publish`, reported as a feed event.
    FeedPublish,
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
        }
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
            let _ = self.jobs.send(Work::Consented(request));
        } else {
            self.refused.push(Completion { handle: prompt.handle, outcome: Outcome::Refused });
        }
    }

    fn receive(&mut self, f: Finished, done: &mut Vec<Completion>) {
        self.outstanding -= 1;
        match f {
            Finished::Done(c) => done.push(c),
            Finished::Ask(prompt, request) => {
                // Queued in handle order among those not yet shown.
                let at = (1..self.prompts.len().max(1))
                    .find(|&i| self.prompts[i].0.handle > prompt.handle)
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
    /// feeds-updated event, SPEC 5); never blocks. Consumes the feed
    /// events so far, like `take_feed_events`.
    pub fn feeds_updated(&mut self) -> bool {
        self.take_feed_events().iter().any(|e| matches!(e, FeedEvent::Entry { .. }))
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
        for r in requests {
            self.outstanding += 1;
            // The worker only stops when this service is dropped.
            let _ = self.jobs.send(Work::Content(r));
        }
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
    /// The page's events (SPEC 11.5), from the reference layout unless the
    /// host gives its own layouter.
    page: PageInput,
}

impl Headless {
    pub fn new(content: Option<ContentService>, script: Script) -> Self {
        Headless { content, script, completions: Vec::new(), feeds_updated: false, page: PageInput::default() }
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
        let completions = std::mem::take(&mut self.completions);
        let controllers = self.script.controllers(machine.frame());
        let input = FrameInput { events: &events, completions: &completions, controllers, ..Default::default() };
        let result = machine.run_frame(&input);
        // The height of a changed page is reported at the next frame.
        self.page.after_frame(machine.page());
        if let Some(content) = &mut self.content {
            content.submit(machine.take_content_requests());
            self.completions = content.wait_finished();
            self.feeds_updated = content.feeds_updated();
        }
        result
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
        (Job::Get { handle, .. } | Job::Put { handle } | Job::Publish { handle }, Done::NotFound) => {
            (handle, Outcome::NotFound)
        }
        (Job::Get { handle, .. } | Job::Put { handle } | Job::Publish { handle } | Job::Find { handle, .. }, _) => {
            (handle, Outcome::Failed)
        }
        (Job::FeedPublish, _) => unreachable!("publishes are reported as feed events"),
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

/// SPEC 9.5: requests that change nothing complete at once without asking
/// (subscribing to a feed already subscribed, unsubscribing from one that
/// is not); an invalid feed key is refused and a blob not in the store is
/// not found, also without asking. The rest are asked about, unless the
/// policy (or the user's answer, `consented`) decides.
fn feed_step(node: &mut Node, consent: Consent, consented: bool, request: &Request, token: Token) -> Step {
    let decided = if consented { Consent::Allow } else { consent };
    let asked = match &request.kind {
        RequestKind::Subscribe { feed } if !valid_feed_key(feed) => return Step::Complete(Outcome::Refused),
        RequestKind::Subscribe { feed } if node.subscriptions().contains(feed) => return Step::Complete(Outcome::Done),
        RequestKind::Unsubscribe { feed } if !node.subscriptions().contains(feed) => {
            return Step::Complete(Outcome::Done);
        }
        RequestKind::Subscribe { feed } => Asked::Subscribe(*feed),
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
        RequestKind::Publish { blob, title } => match node.store().read(blob) {
            Ok(Some(bytes)) => Asked::Publish { title: String::from_utf8_lossy(title).into_owned(), size: bytes.len() as u32 },
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
            RequestKind::Subscribe { feed } => {
                Step::Complete(if node.subscribe(*feed).is_ok() { Outcome::Done } else { Outcome::Failed })
            }
            RequestKind::Unsubscribe { feed } => {
                Step::Complete(if node.unsubscribe(feed).is_ok() { Outcome::Done } else { Outcome::Failed })
            }
            RequestKind::Publish { blob, title } => {
                node.start_publish_blob(token, *blob, &String::from_utf8_lossy(title), now());
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

/// Carries out (or asks about) a subscribe, unsubscribe or publish; false
/// if the service is gone.
fn feed_request(
    node: &mut Node,
    consent: Consent,
    consented: bool,
    request: Request,
    token: Token,
    pending: &mut HashMap<Token, Job>,
    finished: &Sender<Finished>,
) -> bool {
    let handle = request.handle;
    match feed_step(node, consent, consented, &request, token) {
        Step::Complete(outcome) => finished.send(Finished::Done(Completion { handle, outcome })).is_ok(),
        Step::Ask(asked) => finished.send(Finished::Ask(Prompt { handle, asked }, request)).is_ok(),
        Step::Started => {
            pending.insert(token, Job::Publish { handle });
            true
        }
    }
}

fn worker(mut node: Node, consent: Consent, jobs: Receiver<Work>, finished: Sender<Finished>, events: Sender<FeedEvent>) {
    let mut pending: HashMap<Token, Job> = HashMap::new();
    let mut next: Token = 0;
    loop {
        // Block for work only when nothing is waiting on the relay and no
        // feed is followed.
        let wait = if node.busy() || node.connected() {
            Some(Duration::ZERO)
        } else if node.has_relays() {
            Some(IDLE_WAIT)
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
            Some(Work::Content(Request { handle, kind: RequestKind::Identity })) => {
                let outcome = match node.identity_key() {
                    Some(key) => Outcome::Identity { key },
                    None => Outcome::NotFound,
                };
                if finished.send(Finished::Done(Completion { handle, outcome })).is_err() {
                    return;
                }
            }
            Some(Work::Content(
                r @ Request {
                    kind:
                        RequestKind::Subscribe { .. }
                        | RequestKind::Unsubscribe { .. }
                        | RequestKind::Publish { .. }
                        | RequestKind::SetProfile { .. }
                        | RequestKind::SetNickname { .. },
                    ..
                },
            )) => {
                if !feed_request(&mut node, consent, false, r, next, &mut pending, &finished) {
                    return;
                }
            }
            Some(Work::Consented(r)) => {
                if !feed_request(&mut node, consent, true, r, next, &mut pending, &finished) {
                    return;
                }
            }
            Some(Work::Content(Request { handle, kind: RequestKind::Get { id, offset, length } })) => {
                pending.insert(next, Job::Get { handle, offset, length });
                node.start_get(next, id);
            }
            Some(Work::Content(Request { handle, kind: RequestKind::Put { data } })) => {
                pending.insert(next, Job::Put { handle });
                node.start_put(next, &data);
            }
            // SPEC 9.5: answered at once from the node's own state.
            Some(Work::Content(Request { handle, kind: RequestKind::Subscriptions { offset, length } })) => {
                let (total, data) = node.subscription_records(offset, length);
                if finished.send(Finished::Done(Completion { handle, outcome: Outcome::Records { total, data } })).is_err() {
                    return;
                }
            }
            Some(Work::Content(Request { handle, kind: RequestKind::Timeline { offset, length } })) => {
                let (total, data) = node.timeline_records(offset, length);
                if finished.send(Finished::Done(Completion { handle, outcome: Outcome::Records { total, data } })).is_err() {
                    return;
                }
            }
            Some(Work::Content(Request { handle, kind: RequestKind::Names { offset, length } })) => {
                let (total, data) = node.name_records(offset, length);
                if finished.send(Finished::Done(Completion { handle, outcome: Outcome::Records { total, data } })).is_err() {
                    return;
                }
            }
            Some(Work::Content(Request { handle, kind: RequestKind::Find { scope, query, length } })) => {
                pending.insert(next, Job::Find { handle, length });
                node.start_find(next, scope, &query);
            }
            Some(Work::Feed(FeedCommand::Publish(draft))) => {
                pending.insert(next, Job::FeedPublish);
                node.start_publish_article(next, &draft);
            }
            // A subscription list that cannot be saved still applies to
            // this run.
            Some(Work::Feed(FeedCommand::Subscribe(feed))) => drop(node.subscribe(feed)),
            Some(Work::Feed(FeedCommand::Unsubscribe(feed))) => drop(node.unsubscribe(&feed)),
            None => {}
        }
        for (token, done) in node.poll() {
            let sent = match pending.remove(&token) {
                Some(Job::FeedPublish) => events
                    .send(match done {
                        Done::Published { seq, blob } => FeedEvent::Published { seq, blob },
                        done => FeedEvent::PublishFailed(done),
                    })
                    .is_ok(),
                Some(Job::Find { handle, length }) if matches!(done, Done::Found(_)) => {
                    let Done::Found(found) = done else { unreachable!() };
                    let (total, data) = node.find_records(&found, length);
                    let outcome = Outcome::Records { total, data };
                    finished.send(Finished::Done(Completion { handle, outcome })).is_ok()
                }
                Some(job) => finished.send(Finished::Done(completion(job, done))).is_ok(),
                None => true,
            };
            if !sent {
                return;
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

    /// SPEC 9.5: with consent denied, subscribe and publish are refused;
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
                RequestKind::Subscribe { feed: b },
                RequestKind::Subscribe { feed: a },
                RequestKind::Unsubscribe { feed: b },
                RequestKind::Unsubscribe { feed: a },
                RequestKind::Publish { blob: article, title: b"t".to_vec() },
                RequestKind::Publish { blob: [9; 32], title: b"t".to_vec() },
                RequestKind::Identity,
                RequestKind::Subscribe { feed: [0; 32] },
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
            vec![RequestKind::Subscribe { feed: b }, RequestKind::Publish { blob: article, title: "Tïtle".into() }],
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

    /// SPEC 9.5 ops 09h–0Ch: names records; a find with no relay fails;
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
                RequestKind::Subscribe { feed: b },
                RequestKind::Publish { blob: article, title: b"T".to_vec() },
                RequestKind::Identity,
            ],
        );
        assert_eq!(done.len(), 1, "only the identity is answered: {done:?}");
        assert_eq!(service.prompts_waiting(), 2);
        assert_eq!(service.prompt(), Some(&Prompt { handle: 1, asked: Asked::Subscribe(b) }));
        service.answer(false);
        assert_eq!(service.prompt(), Some(&Prompt { handle: 2, asked: Asked::Publish { title: "T".into(), size: 5 } }));
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
}
