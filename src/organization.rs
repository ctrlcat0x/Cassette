use crate::{Track, library::Library};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistMetadata {
    pub folder: Option<String>,
    pub cover: Option<std::path::PathBuf>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SmartRule {
    pub query: String,
    pub artist: String,
    pub album: String,
    pub source: usize,
    pub minimum_plays: u64,
    pub exclude_explicit: bool,
}
impl Library {
    pub fn validate_organization(&mut self) {
        let mut seen = HashSet::new();
        self.folders
            .retain(|f| !f.id.is_empty() && !f.name.trim().is_empty() && seen.insert(f.id.clone()));
        let parents: std::collections::HashMap<_, _> = self
            .folders
            .iter()
            .map(|f| (f.id.clone(), f.parent.clone()))
            .collect();
        for folder in &mut self.folders {
            let mut visited = HashSet::from([folder.id.clone()]);
            let mut parent = folder.parent.clone();
            while let Some(id) = parent {
                if !visited.insert(id.clone()) || !parents.contains_key(&id) {
                    folder.parent = None;
                    break;
                }
                parent = parents.get(&id).cloned().flatten();
            }
        }
        for value in self.playlist_metadata.values_mut() {
            if value.folder.as_ref().is_some_and(|id| !seen.contains(id)) {
                value.folder = None;
            }
        }
        if self
            .active_folder
            .as_ref()
            .is_some_and(|id| !seen.contains(id))
        {
            self.active_folder = None;
        }
        self.refresh_smart_playlists();
    }
    pub fn create_folder(&mut self, name: &str, parent: Option<String>) -> Result<String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            bail!("Enter a folder name between 1 and 100 characters.");
        }
        if parent
            .as_ref()
            .is_some_and(|id| !self.folders.iter().any(|f| &f.id == id))
        {
            bail!("Folder no longer exists");
        }
        let id = format!("folder-{:016x}", rand::random::<u64>());
        self.folders.push(Folder {
            id: id.clone(),
            name: name.into(),
            parent,
        });
        Ok(id)
    }
    pub fn move_folder(&mut self, id: &str, parent: Option<String>) -> Result<()> {
        let mut next = parent.clone();
        let mut seen = HashSet::new();
        while let Some(current) = next {
            if current == id || !seen.insert(current.clone()) {
                bail!("A folder cannot contain itself.");
            }
            let folder = self
                .folders
                .iter()
                .find(|f| f.id == current)
                .ok_or_else(|| anyhow::anyhow!("Folder no longer exists"))?;
            next = folder.parent.clone();
        }
        self.folders
            .iter_mut()
            .find(|f| f.id == id)
            .ok_or_else(|| anyhow::anyhow!("Folder no longer exists"))?
            .parent = parent;
        Ok(())
    }
    pub fn remove_folder(&mut self, id: &str) {
        let parent = self
            .folders
            .iter()
            .find(|f| f.id == id)
            .and_then(|f| f.parent.clone());
        self.folders.retain(|f| f.id != id);
        for folder in &mut self.folders {
            if folder.parent.as_deref() == Some(id) {
                folder.parent = parent.clone();
            }
        }
        for meta in self.playlist_metadata.values_mut() {
            if meta.folder.as_deref() == Some(id) {
                meta.folder = parent.clone();
            }
        }
        if self.active_folder.as_deref() == Some(id) {
            self.active_folder = parent;
        }
    }
    pub fn refresh_smart_playlists(&mut self) {
        self.refresh_smart_playlists_with(&[], &[], &[]);
    }
    pub fn refresh_smart_playlists_with(
        &mut self,
        saved: &[Track],
        liked: &[Track],
        history: &[Track],
    ) {
        for playlist in &mut self.playlists {
            let Some(rule) = &playlist.smart else {
                continue;
            };
            let candidates: Vec<&Track> = match rule.source {
                1 => self.liked.iter().chain(liked).collect(),
                2 => self.local.iter().collect(),
                3 => self.history.iter().chain(history).collect(),
                _ => self
                    .liked
                    .iter()
                    .chain(&self.local)
                    .chain(&self.history)
                    .chain(saved)
                    .chain(liked)
                    .chain(history)
                    .collect(),
            };
            let mut seen = HashSet::new();
            playlist.tracks = candidates
                .into_iter()
                .filter(|t| {
                    t.playable()
                        && seen.insert(t.id.clone())
                        && (!rule.exclude_explicit || t.explicit != Some(true))
                        && t.artist
                            .to_lowercase()
                            .contains(&rule.artist.to_lowercase())
                        && t.album.to_lowercase().contains(&rule.album.to_lowercase())
                        && format!("{} {} {}", t.title, t.artist, t.album)
                            .to_lowercase()
                            .contains(&rule.query.to_lowercase())
                        && self
                            .stats
                            .tracks
                            .get(&t.id)
                            .map_or(0, |s| s.qualified_plays)
                            >= rule.minimum_plays
                })
                .take(10_000)
                .cloned()
                .collect();
        }
    }
    pub fn deduplicate_playlist(&mut self, id: &str) -> usize {
        let Some(playlist) = self.playlists.iter_mut().find(|p| p.id == id) else {
            return 0;
        };
        let before = playlist.tracks.len();
        let mut seen = HashSet::new();
        playlist
            .tracks
            .retain(|t| t.playable() && seen.insert(t.id.clone()));
        before - playlist.tracks.len()
    }
}
