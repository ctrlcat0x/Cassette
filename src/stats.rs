//! Local listening history. Seeking and paused time do not count as listening.
use crate::Track;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Instant};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackStats {
    pub track: Track,
    pub seconds: f64,
    pub starts: u64,
    pub qualified_plays: u64,
    pub last_played: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ListeningStats {
    pub tracks: HashMap<String, TrackStats>,
    pub seconds: f64,
    pub days: Vec<DailyListening>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DailyListening {
    pub day: u64,
    pub seconds: HashMap<String, f64>,
}
impl ListeningStats {
    pub fn started(&mut self, track: &Track) {
        let entry = self.tracks.entry(track.id.clone()).or_default();
        entry.track = track.clone();
        entry.starts += 1;
        entry.last_played = crate::cache::now();
    }
    pub fn listen(&mut self, track: &Track, seconds: f64) {
        if !seconds.is_finite() || seconds <= 0. {
            return;
        }
        // Metadata changes at track start, not at every playback clock tick.
        let entry = if let Some(entry) = self.tracks.get_mut(&track.id) {
            entry
        } else {
            self.tracks
                .entry(track.id.clone())
                .or_insert_with(|| TrackStats {
                    track: track.clone(),
                    ..TrackStats::default()
                })
        };
        entry.seconds += seconds;
        self.seconds += seconds;
        let day = crate::cache::now() / 86400;
        if self.days.last().is_none_or(|record| record.day != day) {
            self.days
                .retain(|record| record.day.saturating_add(400) >= day);
            if self.days.len() >= 400 {
                self.days.remove(0);
            }
            self.days.push(DailyListening {
                day,
                seconds: HashMap::new(),
            });
        }
        if let Some(record) = self.days.last_mut() {
            if let Some(total) = record.seconds.get_mut(&track.id) {
                *total += seconds;
            } else {
                record.seconds.insert(track.id.clone(), seconds);
            }
        }
    }
    pub fn qualify(&mut self, id: &str) -> bool {
        if let Some(entry) = self.tracks.get_mut(id) {
            entry.qualified_plays += 1;
            entry.qualified_plays >= 2
        } else {
            false
        }
    }
    pub fn top_tracks(&self) -> Vec<&TrackStats> {
        let mut tracks: Vec<_> = self.tracks.values().filter(|t| t.seconds > 0.).collect();
        tracks.sort_by(|a, b| {
            b.seconds
                .total_cmp(&a.seconds)
                .then(a.track.id.cmp(&b.track.id))
        });
        tracks
    }
    pub fn ranking(&self, artist: bool) -> Vec<(String, f64)> {
        let mut totals = HashMap::<String, (String, f64, u64)>::new();
        for entry in self.tracks.values() {
            if artist && !entry.track.artists.is_empty() {
                let mut seen = std::collections::HashSet::new();
                for reference in &entry.track.artists {
                    let identity = if reference.id.is_empty() {
                        &reference.name
                    } else {
                        &reference.id
                    };
                    if seen.insert(identity) && !reference.name.is_empty() {
                        let key = if reference.id.is_empty() {
                            format!("name:{}", reference.name)
                        } else {
                            format!("id:{}", reference.id)
                        };
                        let total = totals
                            .entry(key)
                            .or_insert_with(|| (reference.name.clone(), 0., entry.last_played));
                        total.1 += entry.seconds;
                        if entry.last_played >= total.2 {
                            total.0 = reference.name.clone();
                            total.2 = entry.last_played;
                        }
                    }
                }
                continue;
            }
            let label = if artist {
                &entry.track.artist
            } else {
                &entry.track.album
            };
            if !label.is_empty() && entry.seconds > 0. {
                let total = totals
                    .entry(format!("name:{label}"))
                    .or_insert_with(|| (label.clone(), 0., entry.last_played));
                total.1 += entry.seconds;
            }
        }
        let mut totals: Vec<_> = totals
            .into_values()
            .map(|(label, seconds, _)| (label, seconds))
            .collect();
        totals.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        totals
    }
    pub fn period(&self, days: u64) -> Self {
        if days == 0 {
            return Self::default();
        }
        let first = (crate::cache::now() / 86400).saturating_sub(days.saturating_sub(1));
        let mut result = Self::default();
        for record in self
            .days
            .iter()
            .filter(|record| record.day >= first && record.day <= crate::cache::now() / 86400)
        {
            for (id, seconds) in &record.seconds {
                if let Some(original) = self.tracks.get(id) {
                    let entry = result
                        .tracks
                        .entry(id.clone())
                        .or_insert_with(|| TrackStats {
                            track: original.track.clone(),
                            ..Default::default()
                        });
                    entry.seconds += seconds;
                    result.seconds += seconds;
                }
            }
        }
        result
    }
}

pub struct ListeningSession {
    previous: Option<(f64, Instant)>,
    pub seconds: f64,
    pub qualified: bool,
}
impl Default for ListeningSession {
    fn default() -> Self {
        Self {
            previous: None,
            seconds: 0.,
            qualified: false,
        }
    }
}
impl ListeningSession {
    pub fn reset_sample(&mut self) {
        self.previous = None;
    }
    pub fn sample(&mut self, position: f64, now: Instant, playing: bool) -> f64 {
        let before = self.previous.replace((position, now));
        let Some((previous, time)) = before else {
            return 0.;
        };
        let elapsed = now.saturating_duration_since(time).as_secs_f64();
        let delta = position - previous;
        // A discontinuity is a seek, reconnection, or suspend, not time listened.
        let seconds = if playing && delta > 0. && delta <= elapsed + 0.5 && elapsed < 3. {
            delta.min(elapsed)
        } else {
            0.
        };
        self.seconds += seconds;
        seconds
    }
}
