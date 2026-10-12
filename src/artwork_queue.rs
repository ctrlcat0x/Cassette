//! Bounded FIFO within priority classes. Visible covers outrank speculative warmup.
use raag::Track;
use std::collections::VecDeque;

#[derive(Default)]
pub struct PendingArtwork {
    pending: VecDeque<(u8, Track)>,
}
impl PendingArtwork {
    pub fn push(&mut self, track: Track, priority: u8) {
        if let Some((rank, _)) = self
            .pending
            .iter_mut()
            .find(|(_, t)| t.artwork_key() == track.artwork_key())
        {
            *rank = (*rank).min(priority);
        } else if self.pending.len() < 512 {
            self.pending.push_back((priority, track));
        }
    }
    pub fn drain(&mut self, limit: usize) -> Vec<Track> {
        self.pending
            .make_contiguous()
            .sort_by_key(|(rank, _)| *rank);
        self.pending
            .drain(..limit.min(self.pending.len()))
            .map(|(_, track)| track)
            .collect()
    }
}
