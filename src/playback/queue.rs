//! The play queue as pure data: no GTK, no GStreamer, fully unit-tested.

use std::collections::VecDeque;

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
        self.order.iter().map(|&i| self.context[i].clone()).collect()
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
        let first = if self.shuffle {
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
    /// not hold then). Returns false at the end of everything; the current
    /// track then stays, so play starts it again.
    pub fn advance(&mut self, skip: bool) -> bool {
        if !skip && self.repeat == Repeat::One && self.current.is_some() {
            return true;
        }
        let has_next = !self.up_next.is_empty()
            || self.next < self.order.len()
            || (self.repeat != Repeat::Off && !self.order.is_empty());
        if !has_next {
            return false;
        }
        if let Some(current) = self.current.take() {
            self.push_history(current);
        }
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
            }
            self.take_context(0);
        }
        true
    }

    /// Goes back to the previously played track. False if there is none.
    pub fn go_back(&mut self) -> bool {
        let Some(previous) = self.history.pop() else {
            return false;
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

    /// Shuffling reorders what is left of the context; turning it off goes
    /// back to the original order after the current track.
    pub fn set_shuffle(&mut self, shuffle_on: bool) {
        if self.shuffle == shuffle_on {
            return;
        }
        self.shuffle = shuffle_on;
        for entry in &mut self.history {
            entry.context = None;
        }
        if self.order.is_empty() {
            return;
        }
        let next = self.next.min(self.order.len());
        if shuffle_on {
            shuffle(&mut self.order[next..]);
            return;
        }
        // Back to the original order, continuing after the current track.
        let current_original = self
            .current
            .as_ref()
            .and_then(|c| c.context)
            .and_then(|at| self.order.get(at).copied());
        self.order = (0..self.context.len()).collect();
        match current_original {
            Some(original) => {
                if let Some(current) = &mut self.current {
                    current.context = Some(original);
                }
                self.next = original + 1;
            }
            None => self.next = 0,
        }
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
        (1..=n)
            .map(|id| TrackInfo {
                id,
                ..TrackInfo::default()
            })
            .collect()
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
        q.add_to_queue(vec![TrackInfo {
            id: 10,
            ..TrackInfo::default()
        }]);
        q.play_next(vec![TrackInfo {
            id: 9,
            ..TrackInfo::default()
        }]);
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
        q.add_to_queue(vec![TrackInfo {
            id: 9,
            ..TrackInfo::default()
        }]);
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
}
