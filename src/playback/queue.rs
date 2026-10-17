//! The play queue as pure data: no GTK, no GStreamer, fully unit-tested.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::model::TrackInfo;
use crate::settings::Repeat;

use super::Source;

const HISTORY: usize = 100;

#[derive(Clone, Debug)]
struct Entry {
    track: TrackInfo,
    /// Index into `order` when the track came from the context.
    context: Option<usize>,
}

#[derive(Default)]
pub struct Queue {
    /// The context in its original order.
    context: Vec<TrackInfo>,
    /// Play order: indices into `context`.
    order: Vec<usize>,
    /// Index into `order` of the next context track to play.
    next: usize,
    current: Option<Entry>,
    up_next: VecDeque<TrackInfo>,
    history: Vec<Entry>,
    shuffle: bool,
    repeat: Repeat,
    source: Option<Source>,
}

impl Queue {
    pub fn current(&self) -> Option<&TrackInfo> {
        self.current.as_ref().map(|entry| &entry.track)
    }

    pub fn source(&self) -> Option<&Source> {
        self.source.as_ref()
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn repeat(&self) -> Repeat {
        self.repeat
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
    }

    pub fn up_next_len(&self) -> usize {
        self.up_next.len()
    }

    pub fn upcoming(&self) -> Vec<TrackInfo> {
        self.up_next
            .iter()
            .cloned()
            .chain(
                self.order[self.next.min(self.order.len())..]
                    .iter()
                    .map(|&i| self.context[i].clone()),
            )
            .collect()
    }

    pub fn history(&self) -> Vec<TrackInfo> {
        self.history.iter().map(|e| e.track.clone()).collect()
    }

    /// The whole context in play order (for saving the session).
    pub fn context_in_order(&self) -> Vec<TrackInfo> {
        self.order
            .iter()
            .map(|&i| self.context[i].clone())
            .collect()
    }

    /// Replaces the context and makes `tracks[start]` current.
    pub fn set_context(&mut self, tracks: Vec<TrackInfo>, start: usize, source: Source) {
        let start = start.min(tracks.len().saturating_sub(1));
        if let Some(mut previous) = self.current.take() {
            previous.context = None;
            self.push_history(previous);
        }
        for entry in &mut self.history {
            entry.context = None;
        }
        self.context = tracks;
        self.source = Some(source);
        self.order = (0..self.context.len()).collect();
        let first = if self.shuffle && !self.order.is_empty() {
            self.order.swap(0, start);
            shuffle(&mut self.order[1..]);
            0
        } else {
            start
        };
        self.take_context(first);
    }

    fn take_context(&mut self, at: usize) {
        self.current = self.order.get(at).map(|&i| Entry {
            track: self.context[i].clone(),
            context: Some(at),
        });
        self.next = at + 1;
    }

    fn push_history(&mut self, entry: Entry) {
        self.history.push(entry);
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
    }

    pub fn play_next(&mut self, tracks: Vec<TrackInfo>) {
        for track in tracks.into_iter().rev() {
            self.up_next.push_front(track);
        }
    }

    pub fn add_to_queue(&mut self, tracks: Vec<TrackInfo>) {
        self.up_next.extend(tracks);
    }

    /// Moves to the next track. `skip` is a user action (repeat-one does
    /// not hold then). Unavailable tracks are passed over (and dropped).
    /// Returns false at the end of everything; the current track then
    /// stays, so play starts it again.
    pub fn advance(&mut self, skip: bool) -> bool {
        let current_playable = self.current.as_ref().is_some_and(|c| c.track.available);
        if !skip && self.repeat == Repeat::One && current_playable {
            return true;
        }
        if !self.can_advance() {
            return false;
        }
        if let Some(current) = self.current.take()
            && current.track.available
        {
            self.push_history(current);
        }
        loop {
            self.step();
            if self.current.as_ref().is_some_and(|c| c.track.available) {
                return true;
            }
        }
    }

    /// Whether [`Self::advance`] would find a playable track.
    pub fn can_advance(&self) -> bool {
        self.up_next.iter().any(|t| t.available)
            || self.remaining_context().any(|t| t.available)
            || (self.repeat != Repeat::Off && self.context_in_play_order().any(|t| t.available))
    }

    /// Makes the next track current, whatever it is. Only called when
    /// [`Self::can_advance`] holds.
    fn step(&mut self) {
        if let Some(track) = self.up_next.pop_front() {
            self.current = Some(Entry {
                track,
                context: None,
            });
        } else if self.next < self.order.len() {
            self.take_context(self.next);
        } else {
            // Repeat all (or one, when skipping): start the context over.
            if self.shuffle {
                shuffle(&mut self.order);
                for entry in &mut self.history {
                    entry.context = None;
                }
            }
            self.take_context(0);
        }
    }

    fn remaining_context(&self) -> impl Iterator<Item = &TrackInfo> {
        self.order[self.next.min(self.order.len())..]
            .iter()
            .map(|&i| &self.context[i])
    }

    fn context_in_play_order(&self) -> impl Iterator<Item = &TrackInfo> {
        self.order.iter().map(|&i| &self.context[i])
    }

    /// What will play when the current track ends by itself (repeat-one:
    /// the same track), if that is known now. `None` at the end, and when
    /// a shuffled context is about to start over (it is reshuffled then).
    pub fn peek_next(&self) -> Option<&TrackInfo> {
        if self.repeat == Repeat::One
            && let Some(current) = self.current.as_ref().filter(|c| c.track.available)
        {
            return Some(&current.track);
        }
        self.up_next
            .iter()
            .chain(self.remaining_context())
            .find(|t| t.available)
            .or_else(|| {
                if self.repeat == Repeat::Off || self.shuffle {
                    None
                } else {
                    self.context_in_play_order().find(|t| t.available)
                }
            })
    }

    /// If the current track is unavailable, drops it and moves on to the
    /// first playable one (or to nothing). Returns how many were dropped.
    pub fn skip_unavailable(&mut self) -> usize {
        let mut skipped = 0;
        while self.current.as_ref().is_some_and(|c| !c.track.available) {
            skipped += 1;
            self.current = None;
            if !self.can_advance() {
                break;
            }
            self.step();
        }
        skipped
    }

    /// Goes back to the previously played track. False if there is none.
    pub fn go_back(&mut self) -> bool {
        let previous = loop {
            match self.history.pop() {
                Some(entry) if entry.track.available => break entry,
                Some(_) => {}
                None => return false,
            }
        };
        if let Some(current) = self.current.take() {
            match current.context {
                Some(at) => self.next = at,
                None => self.up_next.push_front(current.track),
            }
        }
        if let Some(at) = previous.context {
            self.next = at + 1;
        }
        self.current = Some(previous);
        true
    }

    /// Plays `upcoming()[index]` now. Context tracks before it are skipped;
    /// other up-next tracks stay queued.
    pub fn jump_to(&mut self, index: usize) -> bool {
        let up_next = self.up_next.len();
        let target = if index < up_next {
            self.up_next.remove(index).map(|track| Entry {
                track,
                context: None,
            })
        } else {
            let at = self.next + (index - up_next);
            self.order.get(at).map(|&i| Entry {
                track: self.context[i].clone(),
                context: Some(at),
            })
        };
        let Some(target) = target else {
            return false;
        };
        if let Some(current) = self.current.take() {
            self.push_history(current);
        }
        if let Some(at) = target.context {
            self.next = at + 1;
        }
        self.current = Some(target);
        true
    }

    /// Removes `upcoming()[index]`.
    pub fn remove(&mut self, index: usize) {
        let up_next = self.up_next.len();
        if index < up_next {
            self.up_next.remove(index);
        } else {
            let at = self.next + (index - up_next);
            if at < self.order.len() {
                self.order.remove(at);
                self.fix_history_after_removal(at);
            }
        }
    }

    fn fix_history_after_removal(&mut self, removed: usize) {
        for entry in self.history.iter_mut().chain(self.current.as_mut()) {
            entry.context = match entry.context {
                Some(at) if at > removed => Some(at - 1),
                Some(at) if at == removed => None,
                other => other,
            };
        }
    }

    /// Moves `upcoming()[from]` so it ends up at `upcoming()[to]`. An item
    /// moved across the boundary joins the section it lands in.
    pub fn move_item(&mut self, from: usize, to: usize) {
        let mut upcoming = self.upcoming_tagged();
        if from >= upcoming.len() || to >= upcoming.len() || from == to {
            return;
        }
        let item = upcoming.remove(from);
        upcoming.insert(to, item);
        // The boundary stays where it was in count, unless an item crossed
        // it: the up-next section is everything before the first context
        // item that keeps its original relative order.
        let up_next_len = if from < self.up_next.len() && to >= self.up_next.len() {
            self.up_next.len() - 1
        } else if from >= self.up_next.len() && to < self.up_next.len() {
            self.up_next.len() + 1
        } else {
            self.up_next.len()
        };
        self.up_next = upcoming[..up_next_len]
            .iter()
            .map(|(track, _)| track.clone())
            .collect();
        // Rebuild the remaining context order from what is left.
        let rest: Vec<(TrackInfo, Option<usize>)> = upcoming[up_next_len..].to_vec();
        let mut order = self.order[..self.next.min(self.order.len())].to_vec();
        for (track, original) in rest {
            match original {
                Some(i) => order.push(i),
                None => {
                    self.context.push(track);
                    order.push(self.context.len() - 1);
                }
            }
        }
        self.order = order;
    }

    /// Upcoming tracks with their context index (None for up-next ones).
    fn upcoming_tagged(&self) -> Vec<(TrackInfo, Option<usize>)> {
        self.up_next
            .iter()
            .map(|t| (t.clone(), None))
            .chain(
                self.order[self.next.min(self.order.len())..]
                    .iter()
                    .map(|&i| (self.context[i].clone(), Some(i))),
            )
            .collect()
    }

    pub fn clear_upcoming(&mut self) {
        self.up_next.clear();
        self.order.truncate(self.next.min(self.order.len()));
    }

    /// Shuffling reorders what is left of the context; turning it off puts
    /// what is left back in the original order, continuing after the last
    /// context track played (then wrapping round to the ones before it).
    /// Only the tracks still upcoming are reordered: removed or played ones
    /// do not come back, and the played part (which history points into)
    /// stays as it is.
    pub fn set_shuffle(&mut self, shuffle_on: bool) {
        if self.shuffle == shuffle_on {
            return;
        }
        self.shuffle = shuffle_on;
        let next = self.next.min(self.order.len());
        if shuffle_on {
            shuffle(&mut self.order[next..]);
            return;
        }
        // The last context track taken (current, or before the up-next
        // track that plays now).
        let last = next.checked_sub(1).map(|at| self.order[at]);
        let rest = &mut self.order[next..];
        match last {
            Some(last) => rest.sort_by_key(|&i| (i < last, i)),
            None => rest.sort_unstable(),
        }
    }
}

/// The queue as saved between runs (see `session.rs`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueSnapshot {
    /// The context in its original order.
    pub context: Vec<TrackInfo>,
    /// Play order: indices into `context`.
    pub order: Vec<usize>,
    /// Index into `order` of the next context track.
    pub next: usize,
    pub current: Option<TrackInfo>,
    /// Index into `order` of the current track, when it is from the context.
    pub current_at: Option<usize>,
    pub up_next: Vec<TrackInfo>,
    pub shuffle: bool,
    pub repeat: Repeat,
}

impl Queue {
    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            context: self.context.clone(),
            order: self.order.clone(),
            next: self.next,
            current: self.current.as_ref().map(|c| c.track.clone()),
            current_at: self.current.as_ref().and_then(|c| c.context),
            up_next: self.up_next.iter().cloned().collect(),
            shuffle: self.shuffle,
            repeat: self.repeat,
        }
    }

    /// Rebuilds a saved queue, repairing anything inconsistent.
    pub fn restore(snapshot: QueueSnapshot, source: Option<Source>) -> Self {
        let QueueSnapshot {
            context,
            order,
            next,
            current,
            current_at,
            up_next,
            shuffle,
            repeat,
        } = snapshot;
        let valid = order.len() <= context.len() && order.iter().all(|&i| i < context.len());
        let order = if valid {
            order
        } else {
            (0..context.len()).collect()
        };
        let current_at = current_at.filter(|&at| {
            order
                .get(at)
                .zip(current.as_ref())
                .is_some_and(|(&i, current)| context[i].id == current.id)
        });
        // The next context track comes after the current one.
        let next = match current_at {
            Some(at) if next <= at => at + 1,
            _ => next,
        };
        Self {
            next: next.min(order.len()),
            current: current.map(|track| Entry {
                track,
                context: current_at,
            }),
            context,
            order,
            up_next: up_next.into(),
            history: Vec::new(),
            shuffle,
            repeat,
            source,
        }
    }
}

impl Queue {
    /// Puts back what was upcoming when `snapshot` was taken (the undo of
    /// [`Queue::clear_upcoming`]): the context in its play order and the
    /// up-next list. The current track, the history, shuffle and repeat stay
    /// as they are. When the current track changed since, its link to the
    /// context is dropped (it plays on and the restored tracks follow it).
    pub fn restore_upcoming(&mut self, snapshot: QueueSnapshot) {
        let QueueSnapshot {
            context,
            order,
            next,
            current,
            current_at,
            up_next,
            ..
        } = snapshot;
        if order.iter().any(|&i| i >= context.len()) {
            // Not a snapshot of a consistent queue: requeue what it held.
            let upcoming = up_next.into_iter().chain(
                order
                    .iter()
                    .skip(next)
                    .filter_map(|&i| context.get(i).cloned()),
            );
            self.up_next.extend(upcoming);
            return;
        }
        let same_current = self.current.as_ref().map(|c| c.track.id) == current.map(|t| t.id);
        let context_changed = self.context.len() != context.len()
            || self.context.iter().zip(&context).any(|(a, b)| a.id != b.id);
        if context_changed {
            for entry in &mut self.history {
                entry.context = None;
            }
        }
        if let Some(entry) = &mut self.current {
            entry.context = if same_current { current_at } else { None };
        }
        self.next = next.min(order.len());
        self.context = context;
        self.order = order;
        self.up_next = up_next.into();
    }
}

/// Fisher–Yates with a xorshift generator seeded from the OS.
fn shuffle<T>(items: &mut [T]) {
    let mut seed = [0u8; 8];
    let _ = getrandom::fill(&mut seed);
    let mut state = u64::from_le_bytes(seed) | 1;
    for i in (1..items.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracks(n: u64) -> Vec<TrackInfo> {
        (1..=n).map(track).collect()
    }

    fn source() -> Source {
        Source {
            title: "Test".into(),
            target: None,
        }
    }

    fn ids(list: &[TrackInfo]) -> Vec<u64> {
        list.iter().map(|t| t.id).collect()
    }

    #[test]
    fn plays_in_order_then_ends() {
        let mut q = Queue::default();
        q.set_context(tracks(3), 1, source());
        assert_eq!(q.current().unwrap().id, 2);
        assert_eq!(ids(&q.upcoming()), [3]);
        assert!(q.advance(false));
        assert_eq!(q.current().unwrap().id, 3);
        assert!(!q.advance(false));
        assert_eq!(q.current().unwrap().id, 3);
    }

    #[test]
    fn up_next_plays_first_and_back_returns() {
        let mut q = Queue::default();
        q.set_context(tracks(3), 0, source());
        q.add_to_queue(vec![track(10)]);
        q.play_next(vec![track(9)]);
        assert_eq!(ids(&q.upcoming()), [9, 10, 2, 3]);
        q.advance(true);
        assert_eq!(q.current().unwrap().id, 9);
        q.advance(true);
        q.advance(true);
        assert_eq!(q.current().unwrap().id, 2);
        assert!(q.go_back());
        assert_eq!(q.current().unwrap().id, 10);
        assert_eq!(ids(&q.upcoming()), [2, 3]);
        assert!(q.go_back());
        assert_eq!(q.current().unwrap().id, 9);
        assert_eq!(ids(&q.upcoming()), [10, 2, 3]);
        assert!(q.go_back());
        assert_eq!(q.current().unwrap().id, 1);
        assert_eq!(ids(&q.upcoming()), [9, 10, 2, 3]);
    }

    #[test]
    fn repeat_modes() {
        let mut q = Queue::default();
        q.set_context(tracks(2), 0, source());
        q.set_repeat(Repeat::One);
        assert!(q.advance(false));
        assert_eq!(q.current().unwrap().id, 1);
        assert!(q.advance(true));
        assert_eq!(q.current().unwrap().id, 2);
        q.set_repeat(Repeat::All);
        assert!(q.advance(false));
        assert_eq!(q.current().unwrap().id, 1);
    }

    #[test]
    fn jump_remove_move_clear() {
        let mut q = Queue::default();
        q.set_context(tracks(5), 0, source());
        q.add_to_queue(vec![track(9)]);
        assert_eq!(ids(&q.upcoming()), [9, 2, 3, 4, 5]);
        q.remove(2);
        assert_eq!(ids(&q.upcoming()), [9, 2, 4, 5]);
        q.move_item(3, 1);
        assert_eq!(ids(&q.upcoming()), [9, 5, 2, 4]);
        q.move_item(0, 2);
        assert_eq!(ids(&q.upcoming()), [5, 2, 9, 4]);
        assert_eq!(q.up_next_len(), 0);
        assert!(q.jump_to(2));
        assert_eq!(q.current().unwrap().id, 9);
        assert_eq!(ids(&q.upcoming()), [4]);
        q.clear_upcoming();
        assert!(q.upcoming().is_empty());
    }

    fn unavailable(id: u64) -> TrackInfo {
        TrackInfo {
            id,
            available: false,
            ..TrackInfo::default()
        }
    }

    fn track(id: u64) -> TrackInfo {
        TrackInfo {
            id,
            available: true,
            ..TrackInfo::default()
        }
    }

    #[test]
    fn unavailable_tracks_are_skipped() {
        let mut list = tracks(5);
        list[1].available = false;
        list[2].available = false;
        let mut q = Queue::default();
        q.set_context(list, 0, source());
        assert_eq!(q.peek_next().unwrap().id, 4);
        assert!(q.advance(false));
        assert_eq!(q.current().unwrap().id, 4);
        assert!(q.go_back());
        assert_eq!(q.current().unwrap().id, 1);

        // Starting on an unavailable track moves to the first playable one.
        let mut list = tracks(3);
        list[0].available = false;
        q.set_context(list, 0, source());
        assert_eq!(q.skip_unavailable(), 1);
        assert_eq!(q.current().unwrap().id, 2);

        // Nothing playable ahead: the end.
        q.add_to_queue(vec![unavailable(9)]);
        assert!(q.advance(false));
        assert_eq!(q.current().unwrap().id, 3);
        assert!(!q.can_advance());
        assert!(!q.advance(false));
        assert_eq!(q.peek_next(), None);
    }

    #[test]
    fn peek_matches_advance() {
        let mut q = Queue::default();
        q.set_context(tracks(3), 0, source());
        q.play_next(vec![track(10)]);
        assert_eq!(q.peek_next().unwrap().id, 10);
        q.advance(false);
        assert_eq!(q.peek_next().unwrap().id, 2);
        q.set_repeat(Repeat::One);
        assert_eq!(q.peek_next().unwrap().id, 10);
        q.set_repeat(Repeat::All);
        q.advance(false);
        q.advance(false);
        assert_eq!(q.current().unwrap().id, 3);
        assert_eq!(q.peek_next().unwrap().id, 1);
        assert!(q.advance(false));
        assert_eq!(q.current().unwrap().id, 1);
        // A shuffled context starting over is reshuffled: unknown.
        q.set_shuffle(true);
        q.set_repeat(Repeat::Off);
        while q.advance(false) {}
        q.set_repeat(Repeat::All);
        assert_eq!(q.peek_next(), None);
        assert!(q.can_advance());
    }

    #[test]
    fn snapshot_round_trip() {
        let mut q = Queue::default();
        q.set_shuffle(true);
        q.set_context(tracks(20), 4, source());
        q.advance(false);
        q.add_to_queue(vec![track(30)]);
        q.set_repeat(Repeat::All);
        let snapshot = q.snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();
        let back: QueueSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snapshot);
        let r = Queue::restore(back, Some(source()));
        assert_eq!(r.current(), q.current());
        assert_eq!(ids(&r.upcoming()), ids(&q.upcoming()));
        assert!(r.shuffle());
        assert_eq!(r.repeat(), Repeat::All);
        assert_eq!(r.source(), Some(&source()));

        // Corrupt indices are repaired instead of panicking.
        let mut broken = q.snapshot();
        broken.order = vec![99, 0];
        broken.next = 50;
        broken.current_at = Some(7);
        let r = Queue::restore(broken, None);
        assert_eq!(r.current().unwrap().id, q.current().unwrap().id);
        assert_eq!(r.upcoming().len(), 1);
    }

    #[test]
    fn clear_then_restore_upcoming() {
        let mut q = Queue::default();
        q.set_shuffle(true);
        q.set_context(tracks(10), 2, source());
        q.advance(false);
        q.add_to_queue(vec![track(20), track(21)]);
        let before = ids(&q.upcoming());
        let snapshot = q.snapshot();
        q.clear_upcoming();
        assert!(q.upcoming().is_empty());
        q.restore_upcoming(snapshot);
        assert_eq!(ids(&q.upcoming()), before);
        assert_eq!(q.up_next_len(), 2);
        // The context order is back, not re-added as up next.
        q.advance(true);
        q.advance(true);
        assert_eq!(q.up_next_len(), 0);
        assert_eq!(ids(&q.upcoming()), before[2..]);
        // Going back still finds the context positions.
        assert!(q.go_back());
        assert!(q.go_back());
        assert!(q.go_back());
        assert_eq!(ids(&q.upcoming()).len(), before.len() + 1);
    }

    #[test]
    fn shuffle_keeps_everything_and_unshuffle_restores() {
        let mut q = Queue::default();
        q.set_shuffle(true);
        q.set_context(tracks(50), 10, source());
        assert_eq!(q.current().unwrap().id, 11);
        let mut all = ids(&q.upcoming());
        all.sort();
        assert_eq!(all.len(), 49);
        assert!(!all.contains(&11));
        q.set_shuffle(false);
        assert_eq!(ids(&q.upcoming())[..3], [12, 13, 14]);
    }
    #[test]
    fn unshuffle_keeps_removed_and_cleared_tracks_out() {
        let mut q = Queue::default();
        q.set_context(tracks(5), 0, source());
        q.remove(0);
        q.set_shuffle(true);
        q.set_shuffle(false);
        assert_eq!(ids(&q.upcoming()), [3, 4, 5]);

        let mut q = Queue::default();
        q.set_context(tracks(5), 0, source());
        q.clear_upcoming();
        q.set_shuffle(true);
        q.set_shuffle(false);
        assert!(q.upcoming().is_empty());
    }

    #[test]
    fn unshuffle_during_up_next_continues_the_context() {
        let mut q = Queue::default();
        q.set_context(tracks(5), 0, source());
        q.advance(false);
        q.advance(false);
        q.play_next(vec![track(99)]);
        q.advance(true);
        assert_eq!(q.current().unwrap().id, 99);
        q.set_shuffle(true);
        q.set_shuffle(false);
        assert_eq!(ids(&q.upcoming()), [4, 5]);
        // History still leads back through the context.
        assert!(q.go_back());
        assert_eq!(q.current().unwrap().id, 3);
        assert_eq!(ids(&q.upcoming()), [99, 4, 5]);
    }

    #[test]
    fn unshuffle_wraps_round_to_earlier_tracks() {
        let mut q = Queue::default();
        q.set_shuffle(true);
        q.set_context(tracks(6), 3, source());
        q.set_shuffle(false);
        assert_eq!(ids(&q.upcoming()), [5, 6, 1, 2, 3]);
        q.move_item(0, 4);
        q.set_shuffle(true);
        q.set_shuffle(false);
        assert_eq!(ids(&q.upcoming()), [5, 6, 1, 2, 3]);
    }

    #[test]
    fn moved_in_track_survives_unshuffle() {
        let mut q = Queue::default();
        q.set_context(tracks(3), 0, source());
        q.add_to_queue(vec![track(9)]);
        q.move_item(0, 2);
        assert_eq!(ids(&q.upcoming()), [2, 3, 9]);
        q.set_shuffle(true);
        q.set_shuffle(false);
        assert_eq!(ids(&q.upcoming()), [2, 3, 9]);
    }

    #[test]
    fn shuffled_empty_context_does_not_panic() {
        let mut q = Queue::default();
        q.set_shuffle(true);
        q.set_context(Vec::new(), 0, source());
        assert!(q.current().is_none());
        assert!(q.upcoming().is_empty());
    }

    #[test]
    fn restore_repairs_next_before_current() {
        let mut q = Queue::default();
        q.set_context(tracks(5), 2, source());
        let mut snapshot = q.snapshot();
        snapshot.next = 0;
        let r = Queue::restore(snapshot, None);
        assert_eq!(ids(&r.upcoming()), [4, 5]);
    }

    #[test]
    fn peek_matches_advance_everywhere() {
        for seed in 0..200u64 {
            let mut q = Queue::default();
            let mut list = tracks(6);
            for (i, t) in list.iter_mut().enumerate() {
                if (seed >> i) & 1 == 1 && i % 2 == 1 {
                    t.available = false;
                }
            }
            q.set_context(list, (seed % 6) as usize, source());
            q.set_repeat(match seed % 3 {
                0 => Repeat::All,
                1 => Repeat::One,
                _ => Repeat::Off,
            });
            if seed % 4 == 0 {
                let mut extra = track(50);
                extra.available = seed % 8 == 0;
                q.add_to_queue(vec![extra]);
            }
            q.skip_unavailable();
            for _ in 0..10 {
                let peek = q.peek_next().map(|t| t.id);
                let advanced = q.advance(false);
                if let Some(peek) = peek {
                    assert!(advanced, "seed {seed}");
                    assert_eq!(q.current().unwrap().id, peek, "seed {seed}");
                }
                if !advanced {
                    break;
                }
            }
        }
    }

    #[test]
    fn jump_and_back_keep_consistency() {
        let mut q = Queue::default();
        q.set_context(tracks(5), 0, source());
        q.advance(false);
        q.move_item(2, 0);
        assert_eq!(ids(&q.upcoming()), [5, 3, 4]);
        assert!(q.go_back());
        assert_eq!(ids(&q.upcoming()), [2, 5, 3, 4]);

        let mut q = Queue::default();
        q.set_context(tracks(5), 0, source());
        q.add_to_queue(vec![track(9)]);
        q.jump_to(3);
        assert_eq!(q.current().unwrap().id, 4);
        assert!(q.go_back());
        let upcoming = ids(&q.upcoming());
        let mut unique = upcoming.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), upcoming.len(), "{upcoming:?}");
    }

    #[test]
    fn undo_clear_after_playing_up_next() {
        let mut q = Queue::default();
        q.set_context(tracks(5), 0, source());
        let snapshot = q.snapshot();
        q.clear_upcoming();
        q.add_to_queue(vec![track(9)]);
        q.advance(true);
        q.restore_upcoming(snapshot);
        assert_eq!(ids(&q.upcoming()), [2, 3, 4, 5]);
    }
}
