//! A soak of the relay's limits (NETWORK 5.3): thousands of puts and
//! entries from several connections against small limits, with the
//! relay's clock moved on so that blobs no entry names expire. The store
//! stays within its bound, memory stays bounded, and another connection's
//! gets go on being answered while the soak runs. The numbers, the slowest
//! of those gets among them, are printed (`--nocapture`); how long a get
//! may take is not judged here, on a computer that may be busy: what
//! bounds it is that collection holds the relay up only a batch at a time
//! (`collection_is_incremental`).

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use terse_net::relay::{DEFAULT_TIMEOUT, RelayClient, RelayReply};
use terse_net::{BlobStore, Entry, Identity, blob_id};
use terse_relay::limits::{Rate, Rates};
use terse_relay::{COLLECT_BATCH, KIB, MIB, Policy, Relay};

const WORKERS: usize = 4;
const ROUNDS: usize = 1000;
const FEEDS: usize = 40;
const FEED_BYTES: u64 = 64 * KIB;
const ROOM: u64 = 2 * MIB;

fn rss_kib() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    s.lines().find(|l| l.starts_with("VmRSS:"))?.split_whitespace().nth(1)?.parse().ok()
}

/// Waits for the answer to `req`.
fn wait(c: &mut RelayClient, req: u32) -> RelayReply {
    let deadline = Instant::now() + DEFAULT_TIMEOUT;
    loop {
        if let Some((_, r)) = c.poll().into_iter().find(|(q, _)| *q == req) {
            return r;
        }
        assert!(Instant::now() < deadline, "no answer");
    }
}

/// A little pseudo-random generator, so the run is the same each time.
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn soak() {
    let dir = tempfile::tempdir().unwrap();
    let big = Rate::new(1e9, Duration::from_secs(60));
    let policy = Policy {
        feed_bytes: Some(FEED_BYTES),
        unreferenced_ttl: Some(Duration::from_secs(3600)),
        unreferenced_bytes: ROOM,
        unreferenced_bytes_per_address: ROOM,
        rates: Some(Rates { uploads: big, upload_bytes: big, new_feeds: big, finds: big }),
        ..Policy::unlimited(1024)
    };
    let store_limit = 64 * MIB;
    let relay = Arc::new(Relay::with_policy(BlobStore::open(dir.path(), Some(store_limit)).unwrap(), policy).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let r = relay.clone();
    std::thread::spawn(move || terse_relay::serve(listener, r));
    // A held blob that an observer gets over and over.
    let watched = b"the article everyone reads".to_vec();
    let owner = Identity::from_seed([99; 32]);
    assert_eq!(relay.put_blob(&watched, None).map(|(id, _)| id), Ok(blob_id(&watched)));
    assert_eq!(relay.publish(&Entry::article(owner.feed_key(), 1, 1, &watched, None, "read me").sign(&owner).unwrap().bytes), 1);
    let rss_before = rss_kib();

    let stop = Arc::new(AtomicBool::new(false));
    // Set once the workers are done: the observer counts the gets answered
    // before that.
    let soaked = Arc::new(AtomicBool::new(false));
    let (first_tx, first_rx) = std::sync::mpsc::channel();
    let observer = {
        let (url, stop, soaked, watched) = (url.clone(), stop.clone(), soaked.clone(), watched.clone());
        std::thread::spawn(move || {
            let mut c = RelayClient::websocket(&url, DEFAULT_TIMEOUT);
            c.connect().unwrap();
            let (mut worst, mut gets, mut during) = (Duration::ZERO, 0u32, 0u32);
            while !stop.load(Ordering::Relaxed) {
                let start = Instant::now();
                let req = c.get(&blob_id(&watched));
                assert_eq!(wait(&mut c, req), RelayReply::Got(watched.clone()));
                if !soaked.load(Ordering::SeqCst) {
                    during += 1;
                }
                worst = worst.max(start.elapsed());
                gets += 1;
                if gets == 1 {
                    let _ = first_tx.send(());
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            (worst, gets, during)
        })
    };
    // The observer is connected and served before the soak starts.
    first_rx.recv_timeout(Duration::from_secs(60)).expect("the observer's first get");
    let started = Instant::now();
    let workers: Vec<_> = (0..WORKERS)
        .map(|w| {
            let url = url.clone();
            std::thread::spawn(move || {
                let mut c = RelayClient::websocket(&url, DEFAULT_TIMEOUT);
                c.connect().unwrap();
                let ids: Vec<Identity> = (0..FEEDS / WORKERS).map(|f| Identity::from_seed([(w * 50 + f) as u8; 32])).collect();
                let mut seqs = vec![0u32; ids.len()];
                let mut state = 0x9E37_79B9_7F4A_7C15 ^ (w as u64 + 1);
                let (mut done, mut refused) = (0, 0);
                for round in 0..ROUNDS {
                    let size = 512 + (next(&mut state) % (8 * KIB)) as usize;
                    let mut blob = vec![0u8; size];
                    blob[..8].copy_from_slice(&next(&mut state).to_le_bytes());
                    blob[8..16].copy_from_slice(&(round as u64).to_le_bytes());
                    blob[16] = w as u8;
                    let req = c.put(&blob);
                    match wait(&mut c, req) {
                        RelayReply::Put(_) => done += 1,
                        RelayReply::Refused(_) => refused += 1,
                        r => panic!("{r:?}"),
                    }
                    // Two blobs in three are named by an entry.
                    if round % 3 != 0 {
                        let f = round % ids.len();
                        seqs[f] += 1;
                        let e = Entry::article(ids[f].feed_key(), seqs[f], 1, &blob, None, "soak").sign(&ids[f]).unwrap();
                        let req = c.publish(&e.bytes);
                        match wait(&mut c, req) {
                            RelayReply::Done => done += 1,
                            RelayReply::Refused(_) => refused += 1,
                            r => panic!("{r:?}"),
                        }
                    }
                }
                (done, refused)
            })
        })
        .collect();
    // An hour of the relay's clock passes every 2 s: blobs no entry names
    // expire as the soak runs. The store is checked as it goes.
    let mut worst_store = 0;
    let ticker = {
        let (relay, stop) = (relay.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
                relay.advance_clock(Duration::from_secs(180));
            }
        })
    };
    let mut results = Vec::new();
    for w in workers {
        while !w.is_finished() {
            worst_store = worst_store.max(relay.blobs().used_bytes());
            std::thread::sleep(Duration::from_millis(20));
        }
        results.push(w.join().unwrap());
    }
    soaked.store(true, Ordering::SeqCst);
    let took = started.elapsed();
    relay.advance_clock(Duration::from_secs(7200));
    relay.collect();
    stop.store(true, Ordering::Relaxed);
    ticker.join().unwrap();
    let (worst_get, gets, during) = observer.join().unwrap();
    let rss_after = rss_kib();
    let (requests, refused): (usize, usize) = results.iter().fold((0, 0), |(a, b), (d, r)| (a + d + r, b + r));
    // Every feed holds at most FEED_BYTES; nothing waits once the time is
    // up.
    let bound = FEEDS as u64 * FEED_BYTES + watched.len() as u64;
    eprintln!(
        "soak: {requests} requests ({refused} refused) from {WORKERS} connections in {:.1} s; store at most {} bytes during, {} after (bound {} + room {}); {} feeds, {} entries; observer: {gets} gets ({during} while the workers ran), worst {} ms; RSS {:?} KiB before, {:?} after; {}",
        took.as_secs_f64(),
        worst_store,
        relay.blobs().used_bytes(),
        bound,
        ROOM,
        relay.feeds().feeds().unwrap().len(),
        relay.feeds().feeds().unwrap().iter().map(|f| relay.feeds().seqs(f).unwrap().len()).sum::<usize>(),
        worst_get.as_millis(),
        rss_before,
        rss_after,
        relay.status()
    );
    assert!(relay.blobs().used_bytes() <= bound, "{} > {bound}", relay.blobs().used_bytes());
    // During the run the room's blobs add to it (each counts at least
    // 4 KiB in the room, so its bytes are at most the room).
    assert!(worst_store <= bound + ROOM, "{worst_store}");
    assert_eq!(relay.waiting(), (0, 0));
    for f in relay.feeds().feeds().unwrap() {
        assert!(relay.feed_bytes(&f).unwrap() <= FEED_BYTES);
    }
    // The observer's gets were answered while the workers' puts and
    // publishes went on, not held up until they ended.
    assert!(during >= 1, "{during} gets answered while the workers ran");
    if let (Some(a), Some(b)) = (rss_before, rss_after) {
        assert!(b < a + 64 * 1024, "memory grew from {a} to {b} KiB");
    }
}

/// Collecting many waiting blobs at once holds the relay up only a batch
/// at a time: a batch removes at most `COLLECT_BATCH` blobs (fewer if
/// `COLLECT_BATCH_TIME` passes on the relay's clock, which stands still
/// here, so every batch but the last is whole), and lets go of the lock
/// before the next, so a connection's get is answered between batches
/// while blobs are still waiting. Counted, not timed: on a busy computer
/// a get may take any time.
#[test]
fn collection_is_incremental() {
    let dir = tempfile::tempdir().unwrap();
    let policy = Policy { unreferenced_ttl: Some(Duration::from_secs(3600)), unreferenced_bytes: 256 * MIB, ..Policy::unlimited(1024) };
    let relay = Arc::new(Relay::with_policy(BlobStore::open(dir.path(), None).unwrap(), policy).unwrap());
    relay.freeze_clock();
    let owner = Identity::from_seed([98; 32]);
    let watched = b"held".to_vec();
    relay.put_blob(&watched, None).unwrap();
    relay.publish(&Entry::article(owner.feed_key(), 1, 1, &watched, None, "held").sign(&owner).unwrap().bytes);
    const N: usize = 5000;
    let filled = Instant::now();
    for n in 0..N as u32 {
        relay.put_blob(&n.to_le_bytes(), None).unwrap();
    }
    let fill = filled.elapsed();
    relay.advance_clock(Duration::from_secs(3601));
    let c = relay.connect();
    let get = terse_net::protocol::encode_get(1, &blob_id(b"held"));
    let (mut batches, mut total) = (0, 0);
    let start = Instant::now();
    loop {
        let (done, more) = relay.collect_batch();
        batches += 1;
        total += done;
        assert!(done <= COLLECT_BATCH, "{done}");
        assert_eq!(relay.waiting().1, N - total);
        if !more {
            break;
        }
        assert_eq!(done, COLLECT_BATCH, "only the count ends a batch while the clock stands still");
        // Between batches the lock is free: answered at once on this
        // thread (a lock still held would never be let go).
        let terse_relay::Answer::Reply(r) = relay.respond(&c, &get) else { panic!("no reply") };
        assert_eq!(terse_net::protocol::decode_reply(&r), Some(terse_net::protocol::Reply::Get { req: 1, status: terse_net::protocol::status::DONE, data: watched.clone() }));
    }
    let took = start.elapsed();
    assert_eq!((total, batches), (N, N.div_ceil(COLLECT_BATCH)));
    assert_eq!(relay.waiting(), (0, 0));
    // The whole of it, as the maintenance thread runs it, batch after
    // batch: the same blobs.
    for n in 0..N as u32 {
        relay.put_blob(&n.to_le_bytes(), None).unwrap();
    }
    relay.advance_clock(Duration::from_secs(3601));
    assert_eq!(relay.collect(), N);
    eprintln!(
        "collection: {N} blobs put in {} ms, collected in {batches} batches in {} ms ({:.1} µs each)",
        fill.as_millis(),
        took.as_millis(),
        took.as_secs_f64() * 1e6 / N as f64,
    );
}
