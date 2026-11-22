//! `terse node`: the reference node of LOCAL.md. It speaks NETWORK.md to
//! the relays with the same store, subscriptions, nicknames and relay list
//! as the other commands and terse-host, and serves readers on a Unix
//! domain socket (LOCAL.md 2), so that a program that only shows articles
//! can read through it.
//!
//! Threads: one owns the [`Node`] and answers every request (those of
//! LOCAL.md 4.5 at once, from the node's state; Get of a blob not held,
//! Find, Lookup and Sync when the network has answered, without holding
//! up the others); each connection has a thread that reads and parses its
//! messages and one that writes its replies and pushes, whole messages in
//! order, so a reader that does not read holds up only itself.

use std::process::ExitCode;

use crate::{Args, USAGE};

/// The options of `terse node`.
const OPTIONS: [&str; 8] =
    ["--socket", "--relay", "--store", "--config", "--no-feed-relays", "--auto-download", "--no-prefetch", "--no-program-download"];

pub fn node(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    args.check_known(&OPTIONS)?;
    if !args.positional.is_empty() {
        return Err(USAGE.into());
    }
    imp::run(&args)
}

#[cfg(not(unix))]
mod imp {
    use super::*;

    pub fn run(_args: &Args) -> Result<ExitCode, String> {
        Err("node: not available on this platform (it serves a Unix domain socket, LOCAL.md 2)".into())
    }
}

#[cfg(unix)]
mod imp {
    use std::collections::{BTreeSet, HashMap};
    use std::io::{Read, Write};
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
    use std::time::{Duration, Instant};

    use terse_net::address::{parse_address, parse_hints};
    use terse_net::article_parse::basic_text;
    use terse_net::entry::{self, KIND_ARTICLE, KIND_DOCUMENT, KIND_PROFILE, KIND_PROGRAM};
    use terse_net::identity::valid_feed_key;
    use terse_net::node::Token;
    use terse_net::timeline::Arrivals;
    use terse_net::{BlobId, Done, Entry, FeedEvent, FeedKey, Node, blob_id, local, profile, to_hex};

    use super::*;

    /// The largest message, without its length (LOCAL.md 2).
    pub const MAX_MESSAGE: usize = 2_097_152;
    /// The most records a Timeline reply carries (LOCAL.md 4.3).
    const MAX_TIMELINE: usize = 1024;
    /// Messages waiting to be written to one connection; a reader that
    /// lets more pile up is disconnected.
    const QUEUE: usize = 4096;
    /// The least time between two Changed pushes (LOCAL.md 4.4 asks for
    /// one within about a second of every change).
    const CHANGED_EVERY: Duration = Duration::from_millis(100);
    /// How long the node's thread sleeps while relays are not connected.
    const IDLE_WAIT: Duration = Duration::from_millis(20);

    // Status (LOCAL.md 4).
    const DONE: u8 = 1;
    const NOT_FOUND: u8 = 2;
    const REFUSED: u8 = 3;

    // Message types (LOCAL.md 4).
    const GET: u8 = 0x01;
    const FIND: u8 = 0x06;
    const LOOKUP: u8 = 0x08;
    const HELLO: u8 = 0x20;
    const HELD: u8 = 0x21;
    const TIMELINE: u8 = 0x22;
    const SUBSCRIPTIONS: u8 = 0x23;
    const FEED: u8 = 0x24;
    const SUBSCRIBE: u8 = 0x25;
    const UNSUBSCRIBE: u8 = 0x26;
    const NICKNAME: u8 = 0x27;
    const RELAYS: u8 = 0x28;
    const SET_RELAYS: u8 = 0x29;
    const SYNC: u8 = 0x2a;
    const TITLE: u8 = 0x2b;
    const CHANGED: u8 = 0xc0;
    const LOG: u8 = 0xc1;

    /// Set by SIGINT, SIGTERM and SIGHUP.
    static STOP: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }

    /// The socket's path: `--socket`, else `TERSE_NODE_SOCKET`, else
    /// `$XDG_RUNTIME_DIR/terse/node.sock`, else `/tmp/terse-<uid>/node.sock`
    /// (LOCAL.md 2).
    pub fn socket_path(given: Option<&str>) -> PathBuf {
        if let Some(p) = given {
            return PathBuf::from(p);
        }
        if let Some(p) = std::env::var_os("TERSE_NODE_SOCKET").filter(|p| !p.is_empty()) {
            return PathBuf::from(p);
        }
        match std::env::var_os("XDG_RUNTIME_DIR").filter(|p| !p.is_empty()) {
            Some(dir) => Path::new(&dir).join("terse").join("node.sock"),
            None => PathBuf::from(format!("/tmp/terse-{}", uid())).join("node.sock"),
        }
    }

    fn uid() -> u32 {
        // SAFETY: getuid has no preconditions and cannot fail.
        unsafe { libc::getuid() }
    }

    /// Creates the socket's directory (mode 0700) if need be and checks
    /// that it is the user's and closed to others (LOCAL.md 2).
    fn check_dir(dir: &Path) -> Result<(), String> {
        if !dir.exists() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let meta = std::fs::metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        if !meta.is_dir() {
            return Err(format!("{} is not a directory", dir.display()));
        }
        if meta.uid() != uid() {
            return Err(format!("{} belongs to another user: refusing to use it", dir.display()));
        }
        if meta.mode() & 0o077 != 0 {
            return Err(format!("{} is open to other users (mode {:o}): refusing to use it", dir.display(), meta.mode() & 0o777));
        }
        Ok(())
    }

    /// Listens on `path`: refuses if a node answers there, removes a socket
    /// file nobody listens on, and makes the socket 0600.
    fn listen(path: &Path) -> Result<UnixListener, String> {
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        check_dir(dir)?;
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            if !meta.file_type().is_socket() {
                return Err(format!("{} exists and is not a socket: refusing to replace it", path.display()));
            }
            if UnixStream::connect(path).is_ok() {
                return Err(format!("{}: a node is already running on this socket", path.display()));
            }
            std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
            eprintln!("terse node: removed the stale socket {}", path.display());
        }
        let listener = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(listener)
    }

    /// The peer's user ID, where the system says (LOCAL.md 2 suggests
    /// closing connections from another user).
    #[cfg(target_os = "linux")]
    fn peer_uid(stream: &UnixStream) -> Option<u32> {
        use std::os::fd::AsRawFd;
        let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: cred and len are valid for writes of the sizes given.
        let r = unsafe {
            libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len)
        };
        (r == 0).then_some(cred.uid)
    }

    #[cfg(not(target_os = "linux"))]
    fn peer_uid(_stream: &UnixStream) -> Option<u32> {
        None
    }

    pub fn run(args: &Args) -> Result<ExitCode, String> {
        let path = socket_path(args.get("--socket"));
        // Refuse early, before connecting to any relay, if a node runs there.
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_socket()) && UnixStream::connect(&path).is_ok() {
            return Err(format!("{}: a node is already running on this socket", path.display()));
        }
        let config = args.get("--config").map(PathBuf::from).or_else(terse_net::default_config_dir).ok_or("no config directory (use --config)")?;
        let relays = crate::relay_urls(args, true)?;
        let node = crate::feed::open_node(args, &relays)?;
        let listener = listen(&path)?;
        let ours = std::fs::symlink_metadata(&path).map(|m| (m.dev(), m.ino())).ok();
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: the handler only stores to an atomic.
            unsafe { libc::signal(sig, on_signal as *const () as libc::sighandler_t) };
        }
        let (tx, rx) = mpsc::channel();
        let server = Server::new(node, config, relays);
        std::thread::Builder::new().name("terse-node".into()).spawn(move || server.run(rx)).map_err(|e| e.to_string())?;
        let accept_tx = tx.clone();
        std::thread::Builder::new().name("terse-node-accept".into()).spawn(move || accept(listener, accept_tx)).map_err(|e| e.to_string())?;
        eprintln!("terse node: serving {}", path.display());
        while !STOP.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }
        // Remove the socket file if it is still the one this node made.
        if std::fs::symlink_metadata(&path).ok().map(|m| (m.dev(), m.ino())) == ours {
            let _ = std::fs::remove_file(&path);
        }
        eprintln!("terse node: stopped");
        Ok(ExitCode::SUCCESS)
    }

    fn accept(listener: UnixListener, tx: Sender<Cmd>) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            if peer_uid(&stream).is_some_and(|u| u != uid()) {
                continue;
            }
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let tx = tx.clone();
            let _ = std::thread::Builder::new().name(format!("terse-node-{id}")).spawn(move || connection(id, stream, tx));
        }
    }

    /// What the connection threads tell the node's thread.
    enum Cmd {
        /// A connection said Hello: replies and pushes go to `out`.
        Open { conn: u64, out: SyncSender<Vec<u8>>, stream: UnixStream },
        Request(u64, Request),
        Closed(u64),
    }

    /// A parsed request (LOCAL.md 4.2, 4.3).
    #[derive(Debug)]
    enum Request {
        Get { req: u32, id: BlobId },
        Find { req: u32, scope: u8, query: Vec<u8> },
        Lookup { req: u32, feed: FeedKey, kind: u8, name: Vec<u8> },
        Held { req: u32, id: BlobId },
        Timeline { req: u32, before: u32, max: u16 },
        Subscriptions { req: u32 },
        Feed { req: u32, flags: u8, address: Vec<u8> },
        Subscribe { req: u32, address: Vec<u8> },
        Unsubscribe { req: u32, feed: FeedKey },
        Nickname { req: u32, feed: FeedKey, name: Vec<u8> },
        Relays { req: u32 },
        SetRelays { req: u32, urls: Vec<Vec<u8>> },
        Sync { req: u32, timeout: u32 },
        Title { req: u32, id: BlobId, size: u32 },
    }

    /// Reads a message's fields; `None` when it ends too soon.
    struct Fields<'a>(&'a [u8]);

    impl<'a> Fields<'a> {
        fn take(&mut self, n: usize) -> Option<&'a [u8]> {
            if self.0.len() < n {
                return None;
            }
            let (head, rest) = self.0.split_at(n);
            self.0 = rest;
            Some(head)
        }
        fn u8(&mut self) -> Option<u8> {
            self.take(1).map(|b| b[0])
        }
        fn u16(&mut self) -> Option<u16> {
            self.take(2).map(|b| u16::from_le_bytes(b.try_into().unwrap()))
        }
        fn u32(&mut self) -> Option<u32> {
            self.take(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        }
        fn id(&mut self) -> Option<[u8; 32]> {
            self.take(32).map(|b| b.try_into().unwrap())
        }
        fn short(&mut self) -> Option<Vec<u8>> {
            let n = self.u8()? as usize;
            self.take(n).map(<[u8]>::to_vec)
        }
        /// The rest must be empty.
        fn end<T>(&self, value: T) -> Option<T> {
            self.0.is_empty().then_some(value)
        }
    }

    /// How a message read on a connection is handled.
    enum Parsed {
        Request(Request),
        /// A type this node does not know, or a relay request that is not a
        /// local message: ignored (LOCAL.md 3, 4.2).
        Ignored,
        /// A Hello after the first: the connection is closed (LOCAL.md 4.1).
        Hello,
        /// A known type it cannot parse: the connection is closed.
        Malformed,
    }

    fn parse(msg: &[u8]) -> Parsed {
        let Some((&t, rest)) = msg.split_first() else { return Parsed::Malformed };
        let mut f = Fields(rest);
        let known = matches!(t, GET | FIND | LOOKUP | HELLO..=TITLE);
        if !known {
            return Parsed::Ignored;
        }
        if t == HELLO {
            return Parsed::Hello;
        }
        let Some(req) = f.u32() else { return Parsed::Malformed };
        let parsed = (|| {
            let r = match t {
                GET => Request::Get { req, id: f.id()? },
                FIND => Request::Find { req, scope: f.u8()?, query: f.short()? },
                LOOKUP => Request::Lookup { req, feed: f.id()?, kind: f.u8()?, name: f.short()? },
                HELD => Request::Held { req, id: f.id()? },
                TIMELINE => Request::Timeline { req, before: f.u32()?, max: f.u16()? },
                SUBSCRIPTIONS => Request::Subscriptions { req },
                FEED => Request::Feed { req, flags: f.u8()?, address: f.short()? },
                SUBSCRIBE => Request::Subscribe { req, address: f.short()? },
                UNSUBSCRIBE => Request::Unsubscribe { req, feed: f.id()? },
                NICKNAME => Request::Nickname { req, feed: f.id()?, name: f.short()? },
                RELAYS => Request::Relays { req },
                SET_RELAYS => {
                    let count = f.u8()?;
                    Request::SetRelays { req, urls: (0..count).map(|_| f.short()).collect::<Option<_>>()? }
                }
                SYNC => Request::Sync { req, timeout: f.u32()? },
                TITLE => Request::Title { req, id: f.id()?, size: f.u32()? },
                _ => unreachable!(),
            };
            f.end(r)
        })();
        parsed.map_or(Parsed::Malformed, Parsed::Request)
    }

    /// Reads one message; `None` at the end of the stream, on an error, or
    /// on a length over the limit.
    fn read_message(stream: &mut impl Read) -> Option<Vec<u8>> {
        let mut len = [0u8; 4];
        stream.read_exact(&mut len).ok()?;
        let len = u32::from_le_bytes(len) as usize;
        if len > MAX_MESSAGE {
            return None;
        }
        let mut msg = vec![0; len];
        stream.read_exact(&mut msg).ok()?;
        Some(msg)
    }

    /// A message with its type and `req`.
    fn head(t: u8, req: u32) -> Vec<u8> {
        let mut m = vec![t];
        m.extend_from_slice(&req.to_le_bytes());
        m
    }

    fn status(t: u8, req: u32, status: u8) -> Vec<u8> {
        let mut m = head(t, req);
        m.push(status);
        m
    }

    /// Status 3 and a reason for people (LOCAL.md 4).
    fn refused(t: u8, req: u32, why: &str) -> Vec<u8> {
        let mut m = status(t, req, REFUSED);
        short(&mut m, why.as_bytes());
        m
    }

    /// `n` (1) and the bytes, cut at 255 bytes on a character boundary.
    fn short(m: &mut Vec<u8>, b: &[u8]) {
        let mut n = b.len().min(255);
        while n < b.len() && std::str::from_utf8(&b[..n]).is_err() && n > 0 {
            n -= 1;
        }
        m.push(n as u8);
        m.extend_from_slice(&b[..n]);
    }

    /// One connection: Hello first, then requests to the node's thread;
    /// a writer thread sends what the node's thread queues.
    fn connection(conn: u64, mut stream: UnixStream, tx: Sender<Cmd>) {
        let Ok(write_half) = stream.try_clone() else { return };
        let Ok(closer) = stream.try_clone() else { return };
        let (out, queue) = mpsc::sync_channel::<Vec<u8>>(QUEUE);
        std::thread::spawn(move || writer(write_half, queue));
        // LOCAL.md 4.1: the first message is Hello; a version this node does
        // not speak is refused, and Hello is still awaited.
        loop {
            let Some(msg) = read_message(&mut stream) else { return };
            let mut f = Fields(&msg);
            let hello = (|| {
                (f.u8()? == HELLO).then_some(())?;
                let req = f.u32()?;
                let version = f.u8()?;
                let _token = f.short()?;
                f.end((req, version))
            })();
            let Some((req, version)) = hello else {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return;
            };
            if version != 0 {
                let _ = out.try_send(refused(HELLO | 0x80, req, "this node speaks version 0"));
                continue;
            }
            // Flags bit 0: this node answers Find and Lookup.
            let mut reply = status(HELLO | 0x80, req, DONE);
            reply.extend_from_slice(&[0, 1]);
            let _ = out.try_send(reply);
            break;
        }
        if tx.send(Cmd::Open { conn, out, stream: closer }).is_err() {
            return;
        }
        loop {
            let Some(msg) = read_message(&mut stream) else { break };
            match parse(&msg) {
                Parsed::Request(r) => {
                    if tx.send(Cmd::Request(conn, r)).is_err() {
                        break;
                    }
                }
                Parsed::Ignored => {}
                Parsed::Hello | Parsed::Malformed => break,
            }
        }
        let _ = stream.shutdown(std::net::Shutdown::Both);
        let _ = tx.send(Cmd::Closed(conn));
    }

    fn writer(mut stream: UnixStream, queue: Receiver<Vec<u8>>) {
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        for msg in queue {
            let mut framed = Vec::with_capacity(4 + msg.len());
            framed.extend_from_slice(&(msg.len() as u32).to_le_bytes());
            framed.extend_from_slice(&msg);
            if stream.write_all(&framed).is_err() {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return;
            }
        }
    }

    struct Conn {
        out: SyncSender<Vec<u8>>,
        stream: UnixStream,
    }

    /// A request waiting on the network.
    enum Pending {
        Get { held: bool },
        Find,
        Lookup,
    }

    /// A stored entry, verified once.
    struct Held {
        entry: Entry,
        /// Complete, with its signature.
        bytes: Vec<u8>,
        arrival: u32,
    }

    struct Server {
        node: Node,
        config: PathBuf,
        /// The own relays, in the user's order.
        configured: Vec<String>,
        conns: HashMap<u64, Conn>,
        pending: HashMap<Token, (u64, u32, Pending)>,
        next_token: Token,
        syncs: Vec<(u64, u32, Instant)>,
        arrivals: Arrivals,
        /// Arrival records read so far.
        seen: usize,
        /// Entries by feed and seq, verified (`None`: invalid on disk).
        entries: HashMap<(FeedKey, u32), Option<Held>>,
        /// Kind-1 entries by blob, in arrival order, for Title.
        by_blob: HashMap<BlobId, Vec<(FeedKey, u32)>>,
        /// The valid entries of each feed, by `seq`.
        by_feed: HashMap<FeedKey, BTreeSet<u32>>,
        /// The highest `seq` of each feed's valid profile entries: its
        /// profile in force (NETWORK.md 4.6).
        profiles: HashMap<FeedKey, u32>,
        /// Profile documents read so far (`None`: not held, or not one).
        documents: HashMap<BlobId, Option<terse_net::profile::Document>>,
        /// Whether a blob is in basic text (ARTICLE.md 1.1); blobs never change.
        basic: HashMap<BlobId, bool>,
        dirty: bool,
        last_changed: Instant,
        hinted: BTreeSet<(FeedKey, String)>,
    }

    impl Server {
        fn new(node: Node, config: PathBuf, configured: Vec<String>) -> Self {
            let arrivals = Arrivals::beside_blobs(node.store().dir());
            let mut s = Server {
                node,
                config,
                configured,
                conns: HashMap::new(),
                pending: HashMap::new(),
                next_token: 0,
                syncs: Vec::new(),
                arrivals,
                seen: 0,
                entries: HashMap::new(),
                by_blob: HashMap::new(),
                by_feed: HashMap::new(),
                profiles: HashMap::new(),
                documents: HashMap::new(),
                basic: HashMap::new(),
                dirty: false,
                last_changed: Instant::now(),
                hinted: BTreeSet::new(),
            };
            s.number_unrecorded();
            s
        }

        /// Gives every stored entry an arrival (LOCAL.md 4.3): entries
        /// stored without one (by an older version) are recorded now, by
        /// increasing `seq`, so that their numbers never change after.
        fn number_unrecorded(&mut self) {
            self.refresh();
            let mut missing = Vec::new();
            for feed in self.node.feed_store().feeds().unwrap_or_default() {
                for seq in self.node.feed_store().seqs(&feed).unwrap_or_default() {
                    if !self.entries.contains_key(&(feed, seq)) {
                        missing.push((seq, feed));
                    }
                }
            }
            missing.sort();
            for (seq, feed) in &missing {
                let _ = self.arrivals.record(feed, *seq);
            }
            if !missing.is_empty() {
                eprintln!("terse node: {} entries stored without an arrival record were numbered", missing.len());
            }
            self.refresh();
        }

        /// Reads arrival records added since the last call (by this node or
        /// another process sharing the store) and verifies their entries.
        fn refresh(&mut self) {
            let order: Vec<(FeedKey, u32)> = match self.arrivals.order() {
                Ok(o) => o[self.seen.min(o.len())..].to_vec(),
                Err(_) => return,
            };
            for (feed, seq) in order {
                self.seen += 1;
                let arrival = self.seen as u32;
                let held = self.node.feed_store().get(&feed, seq).ok().flatten().and_then(|b| entry::decode(&b).ok()).and_then(|s| {
                    (s.entry.feed == feed && s.entry.seq == seq).then_some(Held { entry: s.entry, bytes: s.bytes, arrival })
                });
                if let Some(h) = &held {
                    if h.entry.kind == KIND_ARTICLE {
                        self.by_blob.entry(h.entry.blob).or_default().push((feed, seq));
                    }
                    if h.entry.kind == KIND_PROFILE {
                        let best = self.profiles.entry(feed).or_insert(seq);
                        *best = (*best).max(seq);
                    }
                    self.by_feed.entry(feed).or_default().insert(seq);
                }
                self.entries.insert((feed, seq), held);
            }
        }

        fn run(mut self, rx: Receiver<Cmd>) {
            loop {
                let wait = if self.node.busy() || self.node.connected() || self.node.connecting() {
                    Some(Duration::ZERO)
                } else if self.node.has_relays() {
                    Some(IDLE_WAIT)
                } else {
                    let flush = self.dirty.then(|| CHANGED_EVERY.saturating_sub(self.last_changed.elapsed()));
                    let sync = self.syncs.iter().map(|s| s.2.saturating_duration_since(Instant::now())).min();
                    flush.into_iter().chain(sync).min()
                };
                let first = match wait {
                    Some(w) => match rx.recv_timeout(w) {
                        Ok(c) => Some(c),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return,
                    },
                    None => match rx.recv() {
                        Ok(c) => Some(c),
                        Err(_) => return,
                    },
                };
                for cmd in first.into_iter().chain(std::iter::from_fn(|| rx.try_recv().ok())) {
                    self.command(cmd);
                }
                let done = self.node.poll();
                self.finish(done);
                self.events();
                self.check_syncs();
                if self.dirty && self.last_changed.elapsed() >= CHANGED_EVERY {
                    self.dirty = false;
                    self.last_changed = Instant::now();
                    self.push(vec![CHANGED]);
                }
            }
        }

        fn send(&mut self, conn: u64, msg: Vec<u8>) {
            let Some(c) = self.conns.get(&conn) else { return };
            match c.out.try_send(msg) {
                Ok(()) => {}
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                    let _ = c.stream.shutdown(std::net::Shutdown::Both);
                    self.conns.remove(&conn);
                }
            }
        }

        /// Sends a pushed message to every connection (LOCAL.md 4.4).
        fn push(&mut self, msg: Vec<u8>) {
            let ids: Vec<u64> = self.conns.keys().copied().collect();
            for id in ids {
                self.send(id, msg.clone());
            }
        }

        fn log(&mut self, text: &str) {
            eprintln!("terse node: {text}");
            let text = text.as_bytes();
            let n = text.len().min(4096);
            let mut m = vec![LOG];
            m.extend_from_slice(&(n as u16).to_le_bytes());
            m.extend_from_slice(&text[..n]);
            self.push(m);
        }

        fn command(&mut self, cmd: Cmd) {
            match cmd {
                Cmd::Open { conn, out, stream } => {
                    self.conns.insert(conn, Conn { out, stream });
                }
                Cmd::Closed(conn) => {
                    self.conns.remove(&conn);
                    self.syncs.retain(|s| s.0 != conn);
                }
                Cmd::Request(conn, r) => {
                    if let Some(reply) = self.answer(conn, r) {
                        self.send(conn, reply);
                    }
                    // Operations that finished from the store are answered now.
                    let ready = self.node.take_ready();
                    self.finish(ready);
                }
            }
        }

        fn token(&mut self, conn: u64, req: u32, p: Pending) -> Token {
            self.next_token += 1;
            self.pending.insert(self.next_token, (conn, req, p));
            self.next_token
        }

        /// The reply to a request, or `None` if it waits on the network.
        fn answer(&mut self, conn: u64, r: Request) -> Option<Vec<u8>> {
            // Entries stored since the last request, by the node or by another
            // program sharing the store.
            self.refresh();
            Some(match r {
                Request::Get { req, id } => {
                    let held = self.node.store().path(&id).exists();
                    let token = self.token(conn, req, Pending::Get { held });
                    self.node.start_get(token, id);
                    return None;
                }
                Request::Find { req, scope, query } => {
                    if !(1..=3).contains(&scope) || !(1..=profile::MAX_NAME).contains(&query.len()) || std::str::from_utf8(&query).is_err() {
                        return Some(found_reply(req, REFUSED, &[]));
                    }
                    let token = self.token(conn, req, Pending::Find);
                    self.node.start_find(token, scope, &query);
                    return None;
                }
                Request::Lookup { req, feed, kind, name } => {
                    if kind != KIND_PROGRAM || !std::str::from_utf8(&name).is_ok_and(profile::valid_name) {
                        return Some(status(0x88, req, REFUSED));
                    }
                    let token = self.token(conn, req, Pending::Lookup);
                    self.node.start_lookup(token, feed, &name);
                    return None;
                }
                Request::Held { req, id } => match self.held(&id) {
                    Some(blob) => [status(HELD | 0x80, req, DONE), blob].concat(),
                    None => status(HELD | 0x80, req, NOT_FOUND),
                },
                Request::Timeline { req, before, max } => self.timeline(req, before, max),
                Request::Subscriptions { req } => {
                    let feeds: Vec<FeedKey> = self.node.subscriptions().feeds().copied().collect();
                    let mut m = status(SUBSCRIPTIONS | 0x80, req, DONE);
                    let mut records = Vec::new();
                    let mut count = 0u16;
                    for feed in feeds {
                        let r = self.feed_record(&feed);
                        if m.len() + 2 + records.len() + r.len() > MAX_MESSAGE || count == u16::MAX {
                            break;
                        }
                        records.extend(r);
                        count += 1;
                    }
                    m.extend_from_slice(&count.to_le_bytes());
                    m.extend(records);
                    m
                }
                Request::Feed { req, flags, address } => {
                    let address = match std::str::from_utf8(&address).map_err(|_| "not UTF-8".to_string()).and_then(|a| parse_address(a).map_err(|e| e.to_string())) {
                        Ok(a) => a,
                        Err(e) => return Some(refused(FEED | 0x80, req, &e)),
                    };
                    self.feed(req, flags, &address.key)
                }
                Request::Subscribe { req, address } => {
                    let address = match std::str::from_utf8(&address).map_err(|_| "not UTF-8".to_string()).and_then(|a| parse_address(a).map_err(|e| e.to_string())) {
                        Ok(a) => a,
                        Err(e) => return Some(refused(SUBSCRIBE | 0x80, req, &e)),
                    };
                    let new = !self.node.subscriptions().contains(&address.key);
                    let had = self.node.subscriptions().hints(&address.key).map(str::to_string);
                    let hints = (!address.hints.is_empty()).then_some(address.hints.as_str());
                    if let Err(e) = self.node.subscribe_with_hints(address.key, hints) {
                        return Some(refused(SUBSCRIBE | 0x80, req, &e.to_string()));
                    }
                    if new || self.node.subscriptions().hints(&address.key).map(str::to_string) != had {
                        self.dirty = true;
                    }
                    [status(SUBSCRIBE | 0x80, req, DONE), vec![new as u8]].concat()
                }
                Request::Unsubscribe { req, feed } => {
                    if self.node.subscriptions().contains(&feed) {
                        if let Err(e) = self.node.unsubscribe(&feed) {
                            return Some(refused(UNSUBSCRIBE | 0x80, req, &e.to_string()));
                        }
                        self.dirty = true;
                    }
                    status(UNSUBSCRIBE | 0x80, req, DONE)
                }
                Request::Nickname { req, feed, name } => {
                    let t = NICKNAME | 0x80;
                    if !valid_feed_key(&feed) {
                        return Some(refused(t, req, "not a valid feed key (NETWORK.md 4.1)"));
                    }
                    let name = match std::str::from_utf8(&name) {
                        Ok("") => None,
                        Ok(n) if profile::valid_name(n) => Some(n),
                        _ => return Some(refused(t, req, "not a display name (NETWORK.md 4.6)")),
                    };
                    match self.node.set_nickname(feed, name) {
                        Ok(changed) => {
                            self.dirty |= changed;
                            status(t, req, DONE)
                        }
                        Err(e) => refused(t, req, &e.to_string()),
                    }
                }
                Request::Relays { req } => self.relays(req),
                Request::SetRelays { req, urls } => {
                    let t = SET_RELAYS | 0x80;
                    let mut list: Vec<String> = Vec::new();
                    for u in urls {
                        match String::from_utf8(u) {
                            Ok(u) if local::valid_url(&u) => {
                                if !list.contains(&u) {
                                    list.push(u);
                                }
                            }
                            _ => return Some(refused(t, req, "not a relay URL")),
                        }
                    }
                    if list.len() > local::MAX_RELAYS {
                        return Some(refused(t, req, "at most eight relays"));
                    }
                    if let Err(e) = local::write_relays(&self.config, &list) {
                        return Some(refused(t, req, &e));
                    }
                    self.node.set_relays(&list);
                    self.configured = list;
                    self.dirty = true;
                    status(t, req, DONE)
                }
                Request::Sync { req, timeout } => {
                    if !self.node.has_relays() {
                        return Some(refused(SYNC | 0x80, req, "no relay"));
                    }
                    self.syncs.push((conn, req, Instant::now() + Duration::from_millis(timeout.into())));
                    return None;
                }
                Request::Title { req, id, size } => {
                    let title = self.by_blob.get(&id).into_iter().flatten().find_map(|k| {
                        self.entries.get(k).and_then(Option::as_ref).filter(|h| h.entry.size == size).map(|h| h.entry.title.clone())
                    });
                    match title {
                        Some(t) => {
                            let mut m = status(TITLE | 0x80, req, DONE);
                            short(&mut m, t.as_bytes());
                            m
                        }
                        None => status(TITLE | 0x80, req, NOT_FOUND),
                    }
                }
            })
        }

        /// A blob from the store, verified against its ID.
        fn held(&self, id: &BlobId) -> Option<Vec<u8>> {
            self.node.store().read(id).ok().flatten().filter(|b| blob_id(b) == *id)
        }

        /// The name of LOCAL.md 4.3: the nickname, else the display name of
        /// the profile in force, else empty.
        fn name(&self, feed: &FeedKey) -> (String, bool) {
            if let Some(n) = self.node.nicknames().get(feed) {
                return (n.to_string(), true);
            }
            let title = self.profiles.get(feed).and_then(|seq| self.entries.get(&(*feed, *seq))).and_then(Option::as_ref);
            (title.map(|h| h.entry.title.clone()).unwrap_or_default(), false)
        }

        /// The document of the feed's profile in force, if held and valid.
        fn document(&mut self, feed: &FeedKey) -> Option<terse_net::profile::Document> {
            let seq = self.profiles.get(feed)?;
            let blob = self.entries.get(&(*feed, *seq))?.as_ref()?.entry.blob;
            if let Some(Some(d)) = self.documents.get(&blob) {
                return Some(d.clone());
            }
            let doc = self.held(&blob).and_then(|b| terse_net::profile::Document::parse(&b));
            // A document not held yet may be fetched later.
            if doc.is_some() {
                self.documents.insert(blob, doc.clone());
            }
            doc
        }

        /// A record (LOCAL.md 4.3).
        fn record(&mut self, key: (FeedKey, u32), name: &str) -> Option<Vec<u8>> {
            let h = self.entries.get(&key)?.as_ref()?;
            let (arrival, kind, title_basic, blob, size, bytes) =
                (h.arrival, h.entry.kind, basic_text(h.entry.title.as_bytes()), h.entry.blob, h.entry.size, h.bytes.clone());
            let held = std::fs::metadata(self.node.store().path(&blob)).is_ok_and(|m| m.len() == size as u64);
            let mut full = !title_basic;
            if held && kind == KIND_ARTICLE && !full {
                let basic = match self.basic.get(&blob) {
                    Some(&b) => b,
                    None => {
                        let Some(data) = self.held(&blob) else { return None };
                        let b = basic_text(&data);
                        self.basic.insert(blob, b);
                        b
                    }
                };
                full = !basic;
            }
            let mut r = arrival.to_le_bytes().to_vec();
            r.push(held as u8 | (full as u8) << 1);
            short(&mut r, name.as_bytes());
            r.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
            r.extend_from_slice(&bytes);
            Some(r)
        }

        fn timeline(&mut self, req: u32, before: u32, max: u16) -> Vec<u8> {
            let max = (max as usize).min(MAX_TIMELINE);
            let subscribed: BTreeSet<FeedKey> = self.node.subscriptions().feeds().copied().collect();
            let mut picked: Vec<(FeedKey, u32)> = Vec::new();
            let mut list: Vec<(u32, (FeedKey, u32))> = self
                .entries
                .iter()
                .filter_map(|(k, h)| h.as_ref().map(|h| (h.arrival, *k, h.entry.kind)))
                .filter(|(a, k, kind)| subscribed.contains(&k.0) && (*kind == KIND_ARTICLE || *kind == KIND_DOCUMENT) && (before == 0 || *a < before))
                .map(|(a, k, _)| (a, k))
                .collect();
            list.sort_by(|a, b| b.0.cmp(&a.0));
            picked.extend(list.into_iter().take(max).map(|(_, k)| k));
            let mut names: HashMap<FeedKey, String> = HashMap::new();
            let mut m = status(TIMELINE | 0x80, req, DONE);
            let mut records = Vec::new();
            let mut count = 0u16;
            for k in picked {
                let name = names.entry(k.0).or_insert_with(|| self.name(&k.0).0).clone();
                let Some(r) = self.record(k, &name) else { continue };
                if m.len() + 2 + records.len() + r.len() > MAX_MESSAGE {
                    break;
                }
                records.extend(r);
                count += 1;
            }
            m.extend_from_slice(&count.to_le_bytes());
            m.extend(records);
            m
        }

        /// A feed record (LOCAL.md 4.3).
        fn feed_record(&mut self, feed: &FeedKey) -> Vec<u8> {
            let subscribed = self.node.subscriptions().contains(feed);
            let (name, nickname) = self.name(feed);
            let hints = if subscribed { self.node.subscriptions().hints(feed).unwrap_or("").to_string() } else { String::new() };
            let hint_urls: Vec<String> = parse_hints(hints.as_bytes()).unwrap_or_default().iter().map(|h| h.url()).collect();
            let doc = self.document(feed).unwrap_or_default();
            let mut r = feed.to_vec();
            r.push(subscribed as u8 | (nickname as u8) << 1);
            short(&mut r, name.as_bytes());
            short(&mut r, hints.as_bytes());
            for urls in [hint_urls, self.node.hint_relays(feed), doc.relays] {
                let urls: Vec<String> = urls.into_iter().filter(|u| u.len() <= 255).take(255).collect();
                r.push(urls.len() as u8);
                for u in urls {
                    short(&mut r, u.as_bytes());
                }
            }
            let d = doc.description.as_bytes();
            let n = d.len().min(u16::MAX as usize);
            r.extend_from_slice(&(n as u16).to_le_bytes());
            r.extend_from_slice(&d[..n]);
            r
        }

        fn feed(&mut self, req: u32, flags: u8, feed: &FeedKey) -> Vec<u8> {
            let mut m = status(FEED | 0x80, req, DONE);
            m.extend(self.feed_record(feed));
            if flags & 1 == 0 {
                return m;
            }
            let name = self.name(feed).0;
            let mut records: Vec<Vec<u8>> = Vec::new();
            let seqs: Vec<u32> = self.by_feed.get(feed).into_iter().flatten().copied().collect();
            for seq in seqs {
                if let Some(r) = self.record((*feed, seq), &name) {
                    records.push(r);
                }
            }
            // The latest that fit the message (increasing seq).
            let mut size = m.len() + 2;
            let mut first = records.len();
            while first > 0 && size + records[first - 1].len() <= MAX_MESSAGE && records.len() - first < u16::MAX as usize {
                first -= 1;
                size += records[first].len();
            }
            m.extend_from_slice(&((records.len() - first) as u16).to_le_bytes());
            for r in &records[first..] {
                m.extend_from_slice(r);
            }
            m
        }

        fn relays(&self, req: u32) -> Vec<u8> {
            let links = self.node.relay_links();
            let mut list: Vec<(u8, String)> = Vec::new();
            for url in &self.configured {
                let connected = links.iter().any(|l| l.own && &l.url == url && l.connected);
                list.push((1 | (connected as u8) << 2, url.clone()));
            }
            for l in links.iter().filter(|l| !self.configured.contains(&l.url)) {
                list.push(((l.own as u8) | (l.hint as u8) << 1 | (l.connected as u8) << 2, l.url.clone()));
            }
            list.retain(|(_, u)| u.len() <= 255);
            list.truncate(255);
            let mut m = status(RELAYS | 0x80, req, DONE);
            m.push(list.len() as u8);
            for (flags, url) in list {
                m.push(flags);
                short(&mut m, url.as_bytes());
            }
            m
        }

        /// Answers requests that waited on the network.
        fn finish(&mut self, done: Vec<(Token, Done)>) {
            for (token, done) in done {
                let Some((conn, req, p)) = self.pending.remove(&token) else { continue };
                let reply = match (p, done) {
                    (Pending::Get { held }, Done::Got(bytes)) => {
                        self.dirty |= !held;
                        [status(GET | 0x80, req, DONE), bytes].concat()
                    }
                    (Pending::Get { .. }, _) => status(GET | 0x80, req, NOT_FOUND),
                    (Pending::Find, Done::Found(found)) => {
                        let entries: Vec<&[u8]> = found.iter().map(|s| s.bytes.as_slice()).collect();
                        found_reply(req, DONE, &entries)
                    }
                    // No relay answered: refused, as a relay refuses (NETWORK.md 4.7).
                    (Pending::Find, _) => found_reply(req, REFUSED, &[]),
                    (Pending::Lookup, Done::Found(found)) if !found.is_empty() => {
                        self.dirty = true;
                        [status(0x88, req, DONE), found[0].bytes.clone()].concat()
                    }
                    (Pending::Lookup, Done::Refused) => status(0x88, req, REFUSED),
                    (Pending::Lookup, _) => status(0x88, req, NOT_FOUND),
                };
                self.send(conn, reply);
            }
        }

        /// Turns the node's feed events into Changed and Log pushes.
        fn events(&mut self) {
            for e in self.node.take_feed_events() {
                match e {
                    FeedEvent::Entry { .. } | FeedEvent::ProgramDownloaded { .. } | FeedEvent::SubscriptionsChanged => self.dirty = true,
                    FeedEvent::Connected(url) => {
                        self.dirty = true;
                        self.log(&format!("connected to {url}"));
                    }
                    FeedEvent::Disconnected { url, retry_in, why } => {
                        self.dirty = true;
                        let why = why.map(|w| format!(" ({w})")).unwrap_or_default();
                        self.log(&format!("{url}: not connected{why}; trying again in {} s", retry_in.as_secs()));
                    }
                    FeedEvent::SubscribeRefused(key) => self.log(&format!("a relay refused the subscription to {}", to_hex(&key))),
                    FeedEvent::Unreachable => self.log("none of your relays can be reached"),
                    FeedEvent::Subscribed(_) | FeedEvent::Settled | FeedEvent::Busy | FeedEvent::Published { .. } | FeedEvent::PublishFailed(_) => {}
                }
            }
            // A hint in use (NETWORK.md 4.9).
            let feeds: Vec<FeedKey> = self.node.subscriptions().feeds().copied().collect();
            let now: BTreeSet<(FeedKey, String)> = feeds.iter().flat_map(|f| self.node.hint_relays(f).into_iter().map(move |u| (*f, u))).collect();
            if now != self.hinted {
                let new: Vec<(FeedKey, String)> = now.difference(&self.hinted).cloned().collect();
                self.hinted = now;
                for (feed, url) in new {
                    self.log(&format!("{}: not on your relays; following it on {url}, a hint of its address", to_hex(&feed)));
                }
                self.dirty = true;
            }
        }

        /// Answers the Syncs that are done or out of time (LOCAL.md 4.3).
        fn check_syncs(&mut self) {
            if self.syncs.is_empty() {
                return;
            }
            let caught_up = self.node.caught_up();
            let now = Instant::now();
            let (done, waiting): (Vec<_>, Vec<_>) = self.syncs.drain(..).partition(|s| caught_up || now >= s.2);
            self.syncs = waiting;
            for (conn, req, _) in done {
                self.send(conn, status(SYNC | 0x80, req, if caught_up { DONE } else { NOT_FOUND }));
            }
        }
    }

    /// A Find reply (NETWORK.md 4.7).
    fn found_reply(req: u32, st: u8, entries: &[&[u8]]) -> Vec<u8> {
        let mut m = status(0x86, req, st);
        m.push(entries.len().min(255) as u8);
        for e in entries.iter().take(255) {
            m.extend_from_slice(&(e.len() as u16).to_le_bytes());
            m.extend_from_slice(e);
        }
        m
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parsing() {
            assert!(matches!(parse(&[0x23, 1, 0, 0, 0]), Parsed::Request(Request::Subscriptions { req: 1 })));
            // Trailing bytes, or too few: malformed.
            assert!(matches!(parse(&[0x23, 1, 0, 0, 0, 9]), Parsed::Malformed));
            assert!(matches!(parse(&[0x22, 1, 0, 0, 0, 0]), Parsed::Malformed));
            assert!(matches!(parse(&[]), Parsed::Malformed));
            // Relay requests that are not local messages, and unknown types: ignored.
            assert!(matches!(parse(&[0x02, 1, 0, 0, 0, 7]), Parsed::Ignored));
            assert!(matches!(parse(&[0x3f, 1, 0, 0, 0]), Parsed::Ignored));
            assert!(matches!(parse(&[0x20, 1, 0, 0, 0, 0, 0]), Parsed::Hello));
            let mut set = vec![0x29, 2, 0, 0, 0, 2];
            for u in [&b"ws://a"[..], b"ws://b"] {
                set.push(u.len() as u8);
                set.extend_from_slice(u);
            }
            assert!(matches!(parse(&set), Parsed::Request(Request::SetRelays { req: 2, ref urls }) if urls.len() == 2));
        }

        #[test]
        fn short_text_is_cut_on_a_character() {
            let mut m = Vec::new();
            short(&mut m, "é".repeat(200).as_bytes());
            assert_eq!(m[0], 254);
            assert!(std::str::from_utf8(&m[1..]).is_ok());
        }
    }
}
