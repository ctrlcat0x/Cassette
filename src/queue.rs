use crate::Track;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Queue {
    tracks: Vec<Track>,
    order: Vec<usize>,
    cursor: Option<usize>,
    pub repeat: Repeat,
    pub shuffle: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub queue: Queue,
    pub position: f64,
}
impl Session {
    pub fn validate(&mut self) {
        let q = &mut self.queue;
        let mut seen = std::collections::HashSet::new();
        if q.tracks.len() > 10_000
            || q.order.len() != q.tracks.len()
            || q.order
                .iter()
                .any(|i| *i >= q.tracks.len() || !seen.insert(*i))
            || q.cursor.is_some_and(|i| i >= q.order.len())
        {
            *self = Self::default();
        }
        if !self.position.is_finite() {
            self.position = 0.;
        }
        self.position = self.position.max(0.);
        if let Some(track) = self.queue.current().filter(|t| t.duration > 0) {
            self.position = self.position.min(track.duration as f64);
        }
    }
}

impl Queue {
    pub fn current(&self) -> Option<&Track> {
        self.cursor
            .and_then(|ix| self.order.get(ix))
            .and_then(|ix| self.tracks.get(*ix))
    }

    pub fn tracks(&self) -> impl Iterator<Item = &Track> {
        self.order.iter().filter_map(|ix| self.tracks.get(*ix))
    }
    pub fn ordered_len(&self) -> usize {
        self.order.len()
    }
    pub fn track_at(&self, position: usize) -> Option<&Track> {
        self.order
            .get(position)
            .and_then(|index| self.tracks.get(*index))
    }

    pub fn replace(&mut self, tracks: Vec<Track>, selected_id: &str) {
        self.replace_matching(tracks, |track| track.id == selected_id);
    }
    pub fn replace_selected(&mut self, tracks: Vec<Track>, selected: &Track) {
        self.replace_matching(tracks, |track| {
            track.id == selected.id
                && selected
                    .set_video_id
                    .as_ref()
                    .is_none_or(|id| track.set_video_id.as_ref() == Some(id))
        });
    }
    fn replace_matching(&mut self, tracks: Vec<Track>, selected: impl Fn(&Track) -> bool) {
        self.tracks = tracks.into_iter().filter(Track::playable).collect();
        self.order = (0..self.tracks.len()).collect();
        self.cursor = self
            .tracks
            .iter()
            .position(selected)
            .or_else(|| (!self.tracks.is_empty()).then_some(0));
        if self.shuffle {
            self.reshuffle();
        }
    }

    pub fn select(&mut self, id: &str) -> bool {
        let selected = self.order.iter().position(|ix| self.tracks[*ix].id == id);
        if selected.is_some() {
            self.cursor = selected;
            true
        } else {
            false
        }
    }
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }
    pub fn select_at(&mut self, position: usize) -> bool {
        if position < self.order.len() {
            self.cursor = Some(position);
            true
        } else {
            false
        }
    }

    /// Peek using the same order and repeat rules as the eventual transition.
    pub fn next_track(&self, natural_end: bool) -> Option<&Track> {
        let cursor = self.cursor?;
        if natural_end && self.repeat == Repeat::One {
            return self.current();
        }
        let position = if cursor + 1 < self.order.len() {
            cursor + 1
        } else if self.repeat == Repeat::All {
            0
        } else {
            return None;
        };
        self.order
            .get(position)
            .and_then(|index| self.tracks.get(*index))
    }
    pub fn advance(&mut self, natural_end: bool) -> Option<&Track> {
        let cursor = self.cursor?;
        if natural_end && self.repeat == Repeat::One {
            return self.current();
        }
        if cursor + 1 < self.order.len() {
            self.cursor = Some(cursor + 1);
        } else if self.repeat == Repeat::All {
            self.cursor = Some(0);
        } else {
            return None;
        }
        self.current()
    }

    pub fn previous(&mut self) -> Option<&Track> {
        let cursor = self.cursor?;
        self.cursor = Some(if cursor > 0 {
            cursor - 1
        } else if self.repeat == Repeat::All {
            self.order.len().saturating_sub(1)
        } else {
            0
        });
        self.current()
    }

    pub fn toggle_shuffle(&mut self) {
        let current = self
            .cursor
            .and_then(|cursor| self.order.get(cursor))
            .copied();
        self.shuffle = !self.shuffle;
        if self.shuffle {
            self.reshuffle();
        } else {
            self.order.sort_unstable();
            self.cursor =
                current.and_then(|current| self.order.iter().position(|ix| *ix == current));
        }
    }

    fn reshuffle(&mut self) {
        let current = self.cursor.and_then(|ix| self.order.get(ix)).copied();
        self.order.shuffle(&mut rand::rng());
        if let Some(current) = current {
            if let Some(ix) = self.order.iter().position(|item| *item == current) {
                self.order.swap(0, ix);
            }
            self.cursor = Some(0);
        }
    }

    pub fn enqueue(&mut self, track: Track, next: bool) {
        if !track.playable() {
            return;
        }
        let ix = self.tracks.len();
        self.tracks.push(track);
        let position = if next {
            self.cursor.map_or(self.order.len(), |cursor| cursor + 1)
        } else {
            self.order.len()
        };
        self.order.insert(position, ix);
        if self.cursor.is_none() {
            self.cursor = Some(0);
        }
    }

    pub fn remove_at(&mut self, position: usize) -> bool {
        if position >= self.order.len() || self.cursor == Some(position) {
            return false;
        }
        let removed = self.order.remove(position);
        self.tracks.remove(removed);
        for index in &mut self.order {
            if *index > removed {
                *index -= 1;
            }
        }
        if self.cursor.is_some_and(|cursor| position < cursor) {
            self.cursor = self.cursor.map(|cursor| cursor - 1);
        }
        true
    }

    pub fn move_at(&mut self, position: usize, delta: isize) {
        if position >= self.order.len() {
            return;
        }
        let target = position.checked_add_signed(delta);
        if let Some(target) = target.filter(|target| *target < self.order.len()) {
            self.order.swap(position, target);
            self.cursor = self.cursor.map(|cursor| {
                if cursor == position {
                    target
                } else if cursor == target {
                    position
                } else {
                    cursor
                }
            });
        }
    }

    pub fn clear_upcoming(&mut self) {
        if let Some(cursor) = self.cursor {
            self.order.truncate(cursor + 1);
        } else {
            self.order.clear();
        }
        if self.order.len() == self.tracks.len() {
            return;
        }
        // Compact once in O(n), retaining the same occurrence/order/cursor even
        // after shuffle. Hidden removed entries must not retain their strings.
        let mut remap = vec![usize::MAX; self.tracks.len()];
        for index in &self.order {
            remap[*index] = 0;
        }
        let mut old = 0;
        let mut next = 0;
        self.tracks.retain(|_| {
            let keep = remap[old] != usize::MAX;
            if keep {
                remap[old] = next;
                next += 1;
            }
            old += 1;
            keep
        });
        for index in &mut self.order {
            *index = remap[*index];
        }
    }
}
