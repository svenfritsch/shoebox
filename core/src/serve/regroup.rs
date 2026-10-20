//! When the clusters and suggestions are worked out again (the regrouping,
//! `clusters::run`). On a library of 100 000 photos one takes minutes of
//! nearly every core, so it is not started after every decision: naming goes
//! on meanwhile (every request sees the cache through the current decisions,
//! `App::people_view`), and a regrouping starts when it pays off:
//!
//! - `Rules::cards` cards named or otherwise decided (`Why::Card`, one per
//!   request), counted since the last regrouping started;
//! - `Rules::pause` after the last decision (`Card` or `Pause`; each one
//!   starts the pause again);
//! - `Rules::soon` after something only a regrouping shows (`Why::Soon`: an
//!   older decision undone, a rejection taken back, a merge; the person left
//!   the Unnamed page; the cards with a suggestion ran out);
//! - at once after new faces (`Why::Now`).
//!
//! The rules live in the server, so every device sees the same state
//! (`Status`, in `/api/info`). Decisions made while a regrouping runs count
//! towards the next one.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

/// When a regrouping starts; `serve::Options::regroup`.
#[derive(Debug, Clone, Copy)]
pub struct Rules {
    /// Cards named or decided since the last regrouping started.
    pub cards: u32,
    /// Quiet time after the last decision.
    pub pause: Duration,
    /// After something only a regrouping shows.
    pub soon: Duration,
}

impl Default for Rules {
    /// 10 cards, 30 s, 3 s; `SHOEBOX_REGROUP=<cards>,<pause ms>,<soon ms>`
    /// sets others (the browser tests against the real server).
    fn default() -> Rules {
        let mut rules = Rules { cards: 10, pause: Duration::from_secs(30), soon: Duration::from_secs(3) };
        if let Ok(v) = std::env::var("SHOEBOX_REGROUP") {
            let n: Vec<u64> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            if let [cards, pause, soon] = n[..] {
                rules = Rules { cards: cards.max(1) as u32, pause: Duration::from_millis(pause), soon: Duration::from_millis(soon) };
            }
        }
        rules
    }
}

/// Why a change asks for a regrouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Why {
    /// A card (or the faces of one request) named, ignored, marked as not a
    /// face or confirmed: counts towards `Rules::cards`, starts the pause again.
    Card,
    /// A decision that does not count as a card (a rejection, an undo the
    /// page shows at once): starts the pause again.
    Pause,
    /// Something only a regrouping shows: in `Rules::soon`.
    Soon,
    /// Decisions wait and the person left the Unnamed page: `Soon`, or
    /// nothing if none wait.
    Left,
    /// New or forgotten faces: at once.
    Now,
}

#[derive(Default)]
struct State {
    /// Decisions wait for a regrouping.
    pending: bool,
    /// Cards counted since the last regrouping started.
    cards: u32,
    /// When the pause after the last decision ends.
    pause_due: Option<Instant>,
    /// When a `Soon` or `Now` is due.
    soon_due: Option<Instant>,
    /// Cards with a suggestion after the last regrouping: if there were
    /// some and none are left, the next one starts soon.
    suggested: u64,
}

/// What the page needs to show the next regrouping (`/api/info`, `clusters`).
#[derive(Debug, Clone, Copy, Serialize, Default, PartialEq, Eq)]
pub struct Status {
    /// Decisions wait for a regrouping.
    pub pending: bool,
    /// Cards still to name before it starts.
    pub cards_left: u32,
    /// Milliseconds until it starts if nothing else happens (relative, so
    /// the clocks of the devices do not matter).
    pub wait_ms: u64,
    /// The whole pause after a decision (what `wait_ms` counts down from).
    pub pause_ms: u64,
}

pub(super) struct Schedule {
    rules: Rules,
    state: Mutex<State>,
    wake: Condvar,
}

impl Schedule {
    pub(super) fn new(rules: Rules) -> Schedule {
        Schedule { rules, state: Mutex::new(State::default()), wake: Condvar::new() }
    }

    /// A change asks for a regrouping.
    pub(super) fn ask(&self, why: Why) {
        let now = Instant::now();
        let mut s = self.state.lock().unwrap();
        match why {
            Why::Card | Why::Pause => {
                s.pending = true;
                if why == Why::Card {
                    s.cards += 1;
                }
                s.pause_due = Some(now + self.rules.pause);
            }
            Why::Left if !s.pending => return,
            Why::Soon | Why::Left => soon(&mut s, now + self.rules.soon),
            Why::Now => soon(&mut s, now),
        }
        drop(s);
        self.wake.notify_all();
    }

    /// After a card: how many cards are left on the Unnamed page and how many
    /// of them have a suggestion. None left at all, or the suggestions ran
    /// out since the last regrouping: it starts soon.
    pub(super) fn cards_now(&self, cards: u64, suggested: u64) {
        let ran_out = {
            let s = self.state.lock().unwrap();
            s.pending && (cards == 0 || (s.suggested > 0 && suggested == 0))
        };
        if ran_out {
            self.ask(Why::Soon);
        }
    }

    /// Cards with a suggestion after a regrouping.
    pub(super) fn grouped(&self, suggested: u64) {
        self.state.lock().unwrap().suggested = suggested;
    }

    pub(super) fn status(&self) -> Status {
        let s = self.state.lock().unwrap();
        let now = Instant::now();
        match due(&s, &self.rules, now) {
            Some(at) => Status {
                pending: true,
                cards_left: self.rules.cards.saturating_sub(s.cards),
                wait_ms: at.saturating_duration_since(now).as_millis() as u64,
                pause_ms: self.rules.pause.as_millis() as u64,
            },
            _ => Status::default(),
        }
    }

    /// Wait at most `max` for a regrouping to be due. If one is, the
    /// decisions waiting for it are taken (later ones count towards the next
    /// one) and true is returned.
    pub(super) fn wait_due(&self, max: Duration) -> bool {
        let until = Instant::now() + max;
        let mut s = self.state.lock().unwrap();
        loop {
            let now = Instant::now();
            match due(&s, &self.rules, now) {
                Some(at) if at <= now => {
                    let suggested = s.suggested;
                    *s = State { suggested, ..State::default() };
                    return true;
                }
                _ if now >= until => return false,
                at => {
                    let next = at.map_or(until, |at| at.min(until));
                    s = self.wake.wait_timeout(s, next - now).unwrap().0;
                }
            }
        }
    }
}

fn soon(s: &mut State, at: Instant) {
    s.pending = true;
    s.soon_due = Some(s.soon_due.map_or(at, |d| d.min(at)));
}

/// When the regrouping starts: the first of the pause's end, a `Soon`, or
/// now when enough cards were named.
fn due(s: &State, rules: &Rules, now: Instant) -> Option<Instant> {
    if !s.pending {
        return None;
    }
    let cards = (s.cards >= rules.cards).then_some(now);
    [s.pause_due, s.soon_due, cards].into_iter().flatten().min()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules { cards: 3, pause: Duration::from_millis(400), soon: Duration::from_millis(50) }
    }

    #[test]
    fn nothing_waits_at_first() {
        let s = Schedule::new(rules());
        assert_eq!(s.status(), Status::default());
        assert!(!s.wait_due(Duration::from_millis(20)));
    }

    #[test]
    fn enough_cards_start_it_at_once() {
        let s = Schedule::new(rules());
        s.ask(Why::Card);
        s.ask(Why::Card);
        let st = s.status();
        assert!(st.pending);
        assert_eq!(st.cards_left, 1);
        assert!(st.wait_ms > 300 && st.wait_ms <= 400, "{st:?}");
        assert!(!s.wait_due(Duration::from_millis(20)));
        s.ask(Why::Card);
        assert!(s.wait_due(Duration::from_millis(20)));
        assert_eq!(s.status(), Status::default());
    }

    #[test]
    fn each_decision_starts_the_pause_again() {
        let s = Schedule::new(rules());
        s.ask(Why::Card);
        assert!(!s.wait_due(Duration::from_millis(250)));
        s.ask(Why::Pause);
        assert!(s.status().wait_ms > 300);
        assert!(!s.wait_due(Duration::from_millis(250)));
        let t = Instant::now();
        assert!(s.wait_due(Duration::from_millis(1000)));
        assert!(t.elapsed() < Duration::from_millis(300));
    }

    #[test]
    fn leaving_starts_it_soon_only_when_decisions_wait() {
        let s = Schedule::new(rules());
        s.ask(Why::Left);
        assert!(!s.status().pending);
        s.ask(Why::Card);
        s.ask(Why::Left);
        assert!(s.status().wait_ms <= 50);
        // A later decision does not push it back.
        s.ask(Why::Card);
        assert!(s.status().wait_ms <= 50);
        assert!(s.wait_due(Duration::from_millis(200)));
    }

    #[test]
    fn suggestions_that_ran_out_start_it_soon() {
        let s = Schedule::new(rules());
        s.grouped(0);
        s.ask(Why::Card);
        s.cards_now(10, 0);
        assert!(s.status().wait_ms > 300, "none to run out");
        s.cards_now(0, 0);
        assert!(s.status().wait_ms <= 50, "no card left");
        assert!(s.wait_due(Duration::from_millis(200)));
        s.grouped(4);
        s.ask(Why::Card);
        s.cards_now(10, 2);
        assert!(s.status().wait_ms > 300);
        s.cards_now(10, 0);
        assert!(s.status().wait_ms <= 50, "ran out");
    }

    #[test]
    fn now_is_due_at_once() {
        let s = Schedule::new(rules());
        s.ask(Why::Now);
        assert!(s.wait_due(Duration::from_millis(10)));
    }
}
