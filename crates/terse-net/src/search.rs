//! A search kept for more results (NETWORK 4.7, "More results"): the
//! cursor each relay gave, the results a relay returned that are not shown
//! yet, and the results shown, so that each page shows up to 16 results
//! not shown before. [`crate::node::Node`] asks the relays; this is the
//! merging, which needs no network.
//!
//! Each relay's results wait in its queue, in its order, until shown. A
//! page merges the queues by the rules of NETWORK 4.7 (scopes 2 and 3: by
//! rank, the first of each relay in relay order, then the second; scope 1:
//! the highest `seq` of each feed, names that start with the query first,
//! then by feed key) and shows the first 16 not shown before; the rest
//! wait for the next page, so no result a relay returned is lost because
//! another relay returned more. A relay is asked for its next page when
//! its queue holds fewer than 16. For scope 1, a result is shown only once
//! every relay that has a cursor has returned results at or past it in
//! that order (else a relay's next page could hold one that comes first).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::entry::SignedEntry;
use crate::identity::FeedKey;
use crate::profile;
use crate::protocol::{MAX_FIND_RESULTS, SCOPE_FEEDS, SCOPE_PROGRAMS};

/// Searches a node keeps at once; the one used least recently goes first.
pub const MAX_SEARCHES: usize = 64;

/// What a relay is asked next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    /// Its first page.
    First,
    /// The page after the cursor it gave.
    Cursor(Vec<u8>),
    /// Nothing: its list has ended (or it refused, or never answered).
    End,
}

/// How the page in flight was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asked {
    /// The first page, with `c` 0.
    First,
    /// The first page again, ending at the query: a relay from before
    /// paging refuses a find with `c` (NETWORK 4.7).
    Legacy,
    /// The page after a cursor.
    Cursor,
}

/// One relay's part in a search.
#[derive(Clone, Debug)]
pub struct RelayPart {
    pub next: Next,
    pub asked: Option<Asked>,
    /// Its verified results not shown yet, in its order.
    pub queue: VecDeque<SignedEntry>,
    /// Scope 1: where its last result stands in the order.
    pub last: Option<(bool, FeedKey)>,
}

/// A search's state (NETWORK 4.7).
#[derive(Clone, Debug)]
pub struct Search {
    pub scope: u8,
    pub query: Vec<u8>,
    /// By link, in the order of the client's relays.
    pub relays: BTreeMap<u64, RelayPart>,
    /// Asked only once, for one page (no more is asked; finds without `c`).
    pub one_shot: bool,
    /// A page is being asked for.
    pub busy: bool,
    /// Tells a search from the one it replaced under the same key.
    pub generation: u64,
    /// When it was last used, for [`MAX_SEARCHES`].
    pub used: u64,
    shown_entries: HashSet<(FeedKey, u32)>,
    shown_feeds: HashSet<FeedKey>,
    /// Scope 3: the highest `seq` seen of each feed and program name.
    highest: HashMap<(FeedKey, String), u32>,
}

/// Where a profile stands in the order of scope 1.
fn rank_key(query: &[u8], s: &SignedEntry) -> (bool, FeedKey) {
    (!profile::starts_with(s.entry.title.as_bytes(), query), s.entry.feed)
}

impl Search {
    pub fn new(scope: u8, query: &[u8], relays: impl IntoIterator<Item = u64>, one_shot: bool, generation: u64) -> Self {
        let part = RelayPart { next: Next::First, asked: None, queue: VecDeque::new(), last: None };
        Search {
            scope,
            query: query.to_vec(),
            relays: relays.into_iter().map(|l| (l, part.clone())).collect(),
            one_shot,
            busy: false,
            generation,
            used: 0,
            shown_entries: HashSet::new(),
            shown_feeds: HashSet::new(),
            highest: HashMap::new(),
        }
    }

    fn shown(&self, s: &SignedEntry) -> bool {
        if self.scope == SCOPE_FEEDS {
            self.shown_feeds.contains(&s.entry.feed)
        } else {
            self.shown_entries.contains(&(s.entry.feed, s.entry.seq))
        }
    }

    /// A relay's page: its verified results (of the right kind, matching
    /// the query) and its cursor (empty: its list ends). Results already
    /// shown are dropped.
    pub fn add_page(&mut self, lid: u64, results: Vec<SignedEntry>, cursor: Vec<u8>) {
        let scope = self.scope;
        let last = results.last().map(|s| rank_key(&self.query, s));
        if scope == SCOPE_PROGRAMS {
            for s in &results {
                let h = self.highest.entry((s.entry.feed, s.entry.title.clone())).or_insert(0);
                *h = (*h).max(s.entry.seq);
            }
        }
        let fresh: Vec<SignedEntry> = results.into_iter().filter(|s| !self.shown(s)).collect();
        let Some(part) = self.relays.get_mut(&lid) else { return };
        part.queue.extend(fresh);
        part.next = if cursor.is_empty() || self.one_shot { Next::End } else { Next::Cursor(cursor) };
        if last.is_some() {
            part.last = last;
        }
    }

    /// A relay refused its cursor: it is asked again from its first page
    /// (NETWORK 4.7), and what it had returned is forgotten (the results
    /// shown stay shown).
    pub fn restart(&mut self, lid: u64) {
        if let Some(part) = self.relays.get_mut(&lid) {
            part.next = Next::First;
            part.queue.clear();
            part.last = None;
        }
    }

    /// Whether a page could show more: some relay has a cursor (or is to
    /// be asked), or holds results not shown yet.
    pub fn more(&self) -> bool {
        self.relays.values().any(|p| p.next != Next::End || p.queue.iter().any(|s| !self.shown(s)))
    }

    /// The relays to ask for their next page now: those with a cursor (or
    /// a first page to ask) that hold fewer than a page of results.
    pub fn wanted(&self) -> Vec<u64> {
        self.relays.iter().filter(|(_, p)| p.next != Next::End && p.queue.len() < MAX_FIND_RESULTS).map(|(&l, _)| l).collect()
    }

    /// The next page: up to 16 results not shown before, merged by the
    /// rules of NETWORK 4.7, now shown.
    pub fn take_page(&mut self) -> Vec<SignedEntry> {
        let page = if self.scope == SCOPE_FEEDS { self.feeds_page() } else { self.entries_page() };
        for s in &page {
            self.shown_feeds.insert(s.entry.feed);
            self.shown_entries.insert((s.entry.feed, s.entry.seq));
        }
        let (scope, shown_feeds, shown_entries, highest) = (self.scope, &self.shown_feeds, &self.shown_entries, &self.highest);
        for part in self.relays.values_mut() {
            part.queue.retain(|s| {
                let current = scope != SCOPE_PROGRAMS || highest.get(&(s.entry.feed, s.entry.title.clone())) == Some(&s.entry.seq);
                let shown =
                    if scope == SCOPE_FEEDS { shown_feeds.contains(&s.entry.feed) } else { shown_entries.contains(&(s.entry.feed, s.entry.seq)) };
                current && !shown
            });
        }
        page
    }

    /// Scopes 2 and 3: by rank across the relays' queues; for scope 3 the
    /// current versions only (no result of the same feed and name with a
    /// higher `seq`, shown or not).
    fn entries_page(&self) -> Vec<SignedEntry> {
        let queues: Vec<&VecDeque<SignedEntry>> = self.relays.values().map(|p| &p.queue).collect();
        let longest = queues.iter().map(|q| q.len()).max().unwrap_or(0);
        let mut seen = HashSet::new();
        let mut page = Vec::new();
        'ranks: for rank in 0..longest {
            for s in queues.iter().filter_map(|q| q.get(rank)) {
                if self.shown(s) || !seen.insert((s.entry.feed, s.entry.seq)) {
                    continue;
                }
                if self.scope == SCOPE_PROGRAMS && self.highest.get(&(s.entry.feed, s.entry.title.clone())) != Some(&s.entry.seq) {
                    continue;
                }
                page.push(s.clone());
                if page.len() == MAX_FIND_RESULTS {
                    break 'ranks;
                }
            }
        }
        page
    }

    /// Scope 1: the highest `seq` of each feed not shown, in order, up to
    /// where every relay with a cursor has reached (none for a one-shot
    /// search).
    fn feeds_page(&self) -> Vec<SignedEntry> {
        let mut latest: BTreeMap<FeedKey, &SignedEntry> = BTreeMap::new();
        for s in self.relays.values().flat_map(|p| &p.queue).filter(|s| !self.shown(s)) {
            if latest.get(&s.entry.feed).is_none_or(|l| l.entry.seq < s.entry.seq) {
                latest.insert(s.entry.feed, s);
            }
        }
        let bound = self.relays.values().filter(|p| matches!(p.next, Next::Cursor(_))).filter_map(|p| p.last).min();
        let mut list: Vec<((bool, FeedKey), &SignedEntry)> = latest.into_values().map(|s| (rank_key(&self.query, s), s)).collect();
        list.sort_by(|a, b| a.0.cmp(&b.0));
        list.into_iter()
            .take_while(|(k, _)| bound.is_none_or(|b| *k <= b))
            .take(MAX_FIND_RESULTS)
            .map(|(_, s)| s.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{self, Entry, KIND_ARTICLE, KIND_PROFILE, KIND_PROGRAM};
    use crate::identity::Identity;
    use crate::protocol::SCOPE_ENTRIES;

    fn signed(id: &Identity, kind: u8, seq: u32, title: &str) -> SignedEntry {
        let e = Entry { feed: id.feed_key(), seq, time: seq, kind, size: 1, blob: [seq as u8; 32], reply_to: [0; 32], title: title.into() };
        entry::decode(&e.sign(id).unwrap().bytes).unwrap()
    }

    fn titles(page: &[SignedEntry]) -> Vec<String> {
        page.iter().map(|s| s.entry.title.clone()).collect()
    }

    /// Scopes 2 and 3: pages by rank, with no result lost when two relays
    /// return more than a page between them, and none shown twice.
    #[test]
    fn entries_by_rank_across_pages() {
        let (a, b) = (Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
        let mut s = Search::new(SCOPE_ENTRIES, b"n", [1, 2], false, 1);
        let page = |id: &Identity, from: u32, n: u32| (from..from + n).map(|i| signed(id, KIND_ARTICLE, i, &format!("n{i}"))).collect::<Vec<_>>();
        s.add_page(1, page(&a, 1, 16), b"a1".to_vec());
        // Relay 2 returns a result relay 1 also returned.
        let mut two = page(&b, 101, 15);
        two.insert(1, signed(&a, KIND_ARTICLE, 1, "n1"));
        s.add_page(2, two, Vec::new());
        assert_eq!(s.wanted(), Vec::<u64>::new(), "both queues hold a page; relay 2 has ended");
        let first = s.take_page();
        assert_eq!(titles(&first)[..4], ["n1", "n101", "n2", "n3"], "by rank, the repeat dropped");
        assert_eq!(first.len(), 16);
        assert!(s.more());
        assert_eq!(s.wanted(), vec![1], "relay 1 holds fewer than a page and has a cursor");
        s.add_page(1, page(&a, 17, 16), Vec::new());
        let mut all = first.clone();
        while s.more() {
            let p = s.take_page();
            assert!(!p.is_empty() && p.len() <= 16);
            all.extend(p);
        }
        let mut keys: Vec<(FeedKey, u32)> = all.iter().map(|s| (s.entry.feed, s.entry.seq)).collect();
        assert_eq!(keys.len(), 32 + 15, "every result once");
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 32 + 15);
    }

    /// Results already shown are dropped from a relay's later pages, and
    /// from its first page again after it refused a cursor.
    #[test]
    fn shown_results_are_dropped_after_a_restart() {
        let a = Identity::from_seed([1; 32]);
        let mut s = Search::new(SCOPE_ENTRIES, b"n", [1], false, 1);
        let page = |from: u32| (from..from + 16).map(|i| signed(&a, KIND_ARTICLE, i, &format!("n{i}"))).collect::<Vec<_>>();
        s.add_page(1, page(1), b"c".to_vec());
        assert_eq!(s.take_page().len(), 16);
        s.restart(1);
        assert_eq!(s.wanted(), vec![1]);
        assert_eq!(s.relays[&1].next, Next::First);
        s.add_page(1, page(1), b"c".to_vec());
        assert!(s.relays[&1].queue.is_empty(), "all shown");
        s.add_page(1, page(17), Vec::new());
        assert_eq!(titles(&s.take_page())[0], "n17");
        assert!(!s.more());
    }

    /// Scope 3: an older version is dropped when a newer one is seen, even
    /// on another page.
    #[test]
    fn programs_current_versions_only() {
        let a = Identity::from_seed([1; 32]);
        let mut s = Search::new(SCOPE_PROGRAMS, b"tool", [1], false, 1);
        s.add_page(1, vec![signed(&a, KIND_PROGRAM, 2, "tool")], b"c".to_vec());
        s.add_page(1, vec![signed(&a, KIND_PROGRAM, 1, "tool"), signed(&a, KIND_PROGRAM, 3, "toolkit")], Vec::new());
        assert_eq!(s.take_page().iter().map(|s| s.entry.seq).collect::<Vec<_>>(), [2, 3]);
    }

    /// Scope 1: one result per feed, the highest `seq`; a feed shown is
    /// not shown again; nothing past where a relay with a cursor stands.
    #[test]
    fn feeds_in_order_up_to_every_cursor() {
        let mut ids: Vec<Identity> = (0..6u8).map(|i| Identity::from_seed([i + 1; 32])).collect();
        ids.sort_by_key(|i| i.feed_key());
        let p = |i: usize, seq: u32, name: &str| signed(&ids[i], KIND_PROFILE, seq, name);
        let mut s = Search::new(SCOPE_FEEDS, b"ana", [1, 2], false, 1);
        // Relay 1: "ana..." names first, then the others; it has more.
        s.add_page(1, vec![p(3, 1, "ana c"), p(0, 1, "x ana"), p(2, 1, "y ana")], b"c".to_vec());
        // Relay 2 has ended; it holds a newer profile of feed 0.
        s.add_page(2, vec![p(1, 1, "ana b"), p(0, 2, "z ana"), p(5, 1, "w ana")], Vec::new());
        let first = s.take_page();
        assert_eq!(titles(&first), ["ana b", "ana c", "z ana", "y ana"], "feed 5 comes after relay 1's last result");
        s.add_page(1, vec![p(4, 1, "v ana"), p(0, 1, "x ana")], Vec::new());
        assert_eq!(titles(&s.take_page()), ["v ana", "w ana"], "feed 0 is shown already");
        assert!(!s.more());
    }

    /// A one-shot search shows one page and has no more, cursor or not.
    #[test]
    fn one_shot() {
        let a = Identity::from_seed([1; 32]);
        let mut s = Search::new(SCOPE_ENTRIES, b"n", [1], true, 1);
        s.add_page(1, vec![signed(&a, KIND_ARTICLE, 1, "n")], b"c".to_vec());
        assert_eq!(s.take_page().len(), 1);
        assert!(!s.more());
    }
}
