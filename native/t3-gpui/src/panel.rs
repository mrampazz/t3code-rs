//! The right panel's tabs (web: `rightPanelStore.ts`): which surfaces a thread has open, the
//! active one, whether the panel shows, and whether it is maximized. Pure state; `header.rs`
//! renders it and `T3App` runs each surface's loading.

use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Surface {
    Files,
    /// A workspace file beside the explorer; `id` is `file:<relative path>`.
    File(String),
    Diff,
    Terminal,
}

impl Surface {
    /// The web's surface id, also what prefs store.
    pub fn id(&self) -> String {
        match self {
            Surface::Files => "files".into(),
            Surface::File(path) => format!("file:{path}"),
            Surface::Diff => "diff".into(),
            Surface::Terminal => "terminal".into(),
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "files" => Some(Surface::Files),
            "diff" => Some(Surface::Diff),
            "terminal" => Some(Surface::Terminal),
            _ => id
                .strip_prefix("file:")
                .filter(|p| !p.is_empty())
                .map(|p| Surface::File(p.to_owned())),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Panel {
    pub open: bool,
    pub surfaces: Vec<Surface>,
    pub active: Option<Surface>,
    pub maximized: bool,
}

impl Panel {
    /// The surface on screen, None when closed or showing the picker.
    pub fn shown(&self) -> Option<&Surface> {
        self.active.as_ref().filter(|_| self.open)
    }

    pub fn shows(&self, surface: &Surface) -> bool {
        self.shown() == Some(surface)
    }

    /// Adds `surface` when missing and shows it (web: `upsertSurface`). A file takes the
    /// standalone explorer's place, since its tab carries the explorer too (web: `openFile`).
    pub fn open_surface(&mut self, surface: Surface) {
        if let Surface::File(_) = surface
            && let Some(i) = self.surfaces.iter().position(|s| *s == Surface::Files)
        {
            if self.surfaces.contains(&surface) {
                self.surfaces.remove(i);
            } else {
                self.surfaces[i] = surface.clone();
            }
        }
        if !self.surfaces.contains(&surface) {
            self.surfaces.push(surface.clone());
        }
        self.active = Some(surface);
        self.open = true;
    }

    /// Opens `surface`, or closes the panel when it is already the one shown (web: `toggle`).
    pub fn toggle_surface(&mut self, surface: Surface) {
        if self.shows(&surface) {
            self.open = false;
        } else {
            self.open_surface(surface);
        }
    }

    /// Shows or hides the panel; with no tabs it opens on the picker (web: `toggleVisibility`).
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Closes a tab; the one sliding into its place takes over, and the last one closes the
    /// panel (web: `closeSurface`).
    pub fn close(&mut self, surface: &Surface) {
        let Some(index) = self.surfaces.iter().position(|s| s == surface) else {
            return;
        };
        self.surfaces.remove(index);
        if self.surfaces.is_empty() {
            self.open = false;
        }
        if self.active.as_ref() == Some(surface) {
            self.active = self
                .surfaces
                .get(index.min(self.surfaces.len().saturating_sub(1)))
                .cloned();
        }
    }

    /// Drops `surface` before the tab at `to` (web: `moveSurface`).
    pub fn move_surface(&mut self, surface: &Surface, to: usize) {
        let Some(from) = self.surfaces.iter().position(|s| s == surface) else {
            return;
        };
        let moved = self.surfaces.remove(from);
        self.surfaces.insert(to.min(self.surfaces.len()), moved);
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "open": self.open,
            "surfaces": self.surfaces.iter().map(Surface::id).collect::<Vec<_>>(),
            "active": self.active.as_ref().map(Surface::id),
        });
        if self.maximized {
            v["maximized"] = json!(true);
        }
        v
    }

    /// Reads prefs; the one-surface strings saved before tabs ("files", "diff") still load.
    pub fn from_json(v: &Value) -> Self {
        if let Some(id) = v.as_str() {
            let mut panel = Panel::default();
            if let Some(surface) = Surface::parse(id) {
                panel.open_surface(surface);
            }
            return panel;
        }
        let surfaces: Vec<Surface> = v["surfaces"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| Surface::parse(s.as_str()?))
            .collect();
        let active = v["active"]
            .as_str()
            .and_then(Surface::parse)
            .filter(|s| surfaces.contains(s));
        Panel {
            open: v["open"] == true,
            surfaces,
            active,
            maximized: v["maximized"] == true,
        }
    }

    /// Nothing worth saving: closed with no tabs.
    pub fn is_empty(&self) -> bool {
        !self.open && self.surfaces.is_empty() && !self.maximized
    }
}

#[cfg(test)]
mod tests {
    use super::{Panel, Surface};
    use serde_json::json;

    fn file(p: &str) -> Surface {
        Surface::File(p.into())
    }

    #[test]
    fn opens_and_toggles_like_the_web() {
        let mut panel = Panel::default();
        panel.toggle();
        assert!(
            panel.open && panel.shown().is_none(),
            "empty panel shows the picker"
        );
        panel.open_surface(Surface::Diff);
        panel.open_surface(Surface::Files);
        assert_eq!(panel.surfaces, [Surface::Diff, Surface::Files]);
        assert!(panel.shows(&Surface::Files));
        panel.toggle_surface(Surface::Diff);
        assert!(panel.shows(&Surface::Diff));
        panel.toggle_surface(Surface::Diff);
        assert!(!panel.open && panel.active == Some(Surface::Diff));
        panel.toggle();
        assert!(panel.shows(&Surface::Diff), "reopening shows the last tab");
    }

    #[test]
    fn a_file_replaces_the_standalone_explorer() {
        let mut panel = Panel::default();
        panel.open_surface(Surface::Diff);
        panel.open_surface(Surface::Files);
        panel.open_surface(file("src/a.rs"));
        assert_eq!(panel.surfaces, [Surface::Diff, file("src/a.rs")]);
        panel.open_surface(file("src/b.rs"));
        assert_eq!(
            panel.surfaces,
            [Surface::Diff, file("src/a.rs"), file("src/b.rs")]
        );
        panel.open_surface(Surface::Files);
        panel.open_surface(file("src/a.rs"));
        assert_eq!(
            panel.surfaces,
            [Surface::Diff, file("src/a.rs"), file("src/b.rs")]
        );
        assert!(panel.shows(&file("src/a.rs")));
    }

    #[test]
    fn closing_activates_the_neighbor_and_the_last_closes_the_panel() {
        let mut panel = Panel::default();
        for s in [Surface::Diff, Surface::Files, Surface::Terminal] {
            panel.open_surface(s);
        }
        panel.active = Some(Surface::Files);
        panel.close(&Surface::Files);
        assert_eq!(panel.active, Some(Surface::Terminal));
        panel.close(&Surface::Terminal);
        assert_eq!(panel.active, Some(Surface::Diff));
        panel.close(&Surface::Diff);
        assert!(!panel.open && panel.active.is_none());
    }

    #[test]
    fn moves_tabs() {
        let mut panel = Panel::default();
        for s in [Surface::Diff, Surface::Files, Surface::Terminal] {
            panel.open_surface(s);
        }
        panel.move_surface(&Surface::Terminal, 0);
        assert_eq!(
            panel.surfaces,
            [Surface::Terminal, Surface::Diff, Surface::Files]
        );
        panel.move_surface(&Surface::Terminal, 9);
        assert_eq!(
            panel.surfaces,
            [Surface::Diff, Surface::Files, Surface::Terminal]
        );
    }

    #[test]
    fn round_trips_prefs_and_reads_the_old_format() {
        let mut panel = Panel::default();
        panel.open_surface(Surface::Diff);
        panel.open_surface(file("a b/c.rs"));
        panel.maximized = true;
        assert_eq!(Panel::from_json(&panel.to_json()), panel);
        let old = Panel::from_json(&json!("files"));
        assert!(old.shows(&Surface::Files) && old.surfaces == [Surface::Files]);
        let junk = Panel::from_json(&json!({"surfaces": ["nope", "diff"], "active": "nope"}));
        assert_eq!((junk.surfaces.len(), junk.active), (1, None));
    }
}
