//! Carries out content requests (SPEC 7) and follows feeds (NETWORK 4.4) on a
//! background thread, so the frame loop only ever submits requests and
//! collects completions and feed events without blocking. Blocking waits are
//! for headless and command-line use.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use terse_core::{Completion, Event, FrameInput, FrameMachine, FrameResult, Outcome, Request, RequestKind, Script};

use crate::identity::FeedKey;
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
    Feed(FeedCommand),
}

pub struct ContentService {
    jobs: Sender<Work>,
    finished: Receiver<Completion>,
    events: Receiver<FeedEvent>,
    /// Requests submitted and not yet returned by `take_finished`/`wait_finished`.
    outstanding: usize,
    /// The node has a relay and subscriptions.
    following: bool,
}

/// What the worker needs to turn a `Done` into a completion.
enum Job {
    Get { handle: u8, offset: u32, length: u16 },
    Put { handle: u8 },
    Publish,
}

impl ContentService {
    pub fn start(node: Node) -> Self {
        let following = node.following();
        let (jobs, job_rx) = mpsc::channel();
        let (done_tx, finished) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        thread::Builder::new()
            .name("terse-content".into())
            .spawn(move || worker(node, job_rx, done_tx, event_tx))
            .expect("spawn content thread");
        ContentService { jobs, finished, events, outstanding: 0, following }
    }

    /// Whether the node follows feeds through a relay.
    pub fn following(&self) -> bool {
        self.following
    }

    /// Whether a new feed entry was stored since the last call (the
    /// feeds-updated event, SPEC 5.1); never blocks. Consumes the feed
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
                Some(FeedEvent::Disconnected { .. }) => return Err("cannot reach the relay".into()),
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
        let done: Vec<Completion> = self.finished.try_iter().collect();
        self.outstanding -= done.len();
        done
    }

    /// Blocks until every submitted request has finished (each is bounded by
    /// the relay timeout). For headless runs that must not depend on timing.
    pub fn wait_finished(&mut self) -> Vec<Completion> {
        let mut done = Vec::with_capacity(self.outstanding);
        while self.outstanding > 0 {
            match self.finished.recv() {
                Ok(c) => {
                    done.push(c);
                    self.outstanding -= 1;
                }
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
/// frame (the initial sync) are not announced.
pub struct Headless {
    content: Option<ContentService>,
    script: Script,
    completions: Vec<Completion>,
    feeds_updated: bool,
}

impl Headless {
    pub fn new(content: Option<ContentService>, script: Script) -> Self {
        Headless { content, script, completions: Vec::new(), feeds_updated: false }
    }

    /// Runs the machine's next frame, then carries out the requests it made.
    pub fn frame(&mut self, machine: &mut impl FrameMachine) -> FrameResult {
        let mut events = self.script.events(machine.frame()).to_vec();
        if std::mem::take(&mut self.feeds_updated) {
            events.push(Event::feeds_updated());
        }
        let completions = std::mem::take(&mut self.completions);
        let controllers = self.script.controllers(machine.frame());
        let input = FrameInput { events: &events, completions: &completions, controllers, ..Default::default() };
        let result = machine.run_frame(&input);
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
        (Job::Get { handle, .. } | Job::Put { handle }, Done::NotFound) => (handle, Outcome::NotFound),
        (Job::Get { handle, .. } | Job::Put { handle }, _) => (handle, Outcome::Failed),
        (Job::Publish, _) => unreachable!("publishes are reported as feed events"),
    };
    Completion { handle, outcome }
}

fn worker(mut node: Node, jobs: Receiver<Work>, finished: Sender<Completion>, events: Sender<FeedEvent>) {
    let mut pending: HashMap<Token, Job> = HashMap::new();
    let mut next: Token = 0;
    loop {
        // Block for work only when nothing is waiting on the relay and no
        // feed is followed.
        let wait = if node.busy() || node.connected() {
            Some(Duration::ZERO)
        } else if node.following() {
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
            Some(Work::Content(Request { handle, kind: RequestKind::Get { id, offset, length } })) => {
                pending.insert(next, Job::Get { handle, offset, length });
                node.start_get(next, id);
            }
            Some(Work::Content(Request { handle, kind: RequestKind::Put { data } })) => {
                pending.insert(next, Job::Put { handle });
                node.start_put(next, &data);
            }
            // SPEC 7.7: answered at once from the node's own state.
            Some(Work::Content(Request { handle, kind: RequestKind::Subscriptions { offset, length } })) => {
                let (total, data) = node.subscription_records(offset, length);
                if finished.send(Completion { handle, outcome: Outcome::Records { total, data } }).is_err() {
                    return;
                }
            }
            Some(Work::Content(Request { handle, kind: RequestKind::Timeline { offset, length } })) => {
                let (total, data) = node.timeline_records(offset, length);
                if finished.send(Completion { handle, outcome: Outcome::Records { total, data } }).is_err() {
                    return;
                }
            }
            Some(Work::Feed(FeedCommand::Publish(draft))) => {
                pending.insert(next, Job::Publish);
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
                Some(Job::Publish) => events
                    .send(match done {
                        Done::Published { seq, blob } => FeedEvent::Published { seq, blob },
                        done => FeedEvent::PublishFailed(done),
                    })
                    .is_ok(),
                Some(job) => finished.send(completion(job, done)).is_ok(),
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
}
