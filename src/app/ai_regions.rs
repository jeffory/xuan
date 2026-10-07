//! The AI Region tool without an open dialog: boxes on the canvas, each with
//! the region action (its verb) and the values it will run with. Boxes belong
//! to their document for the session; they are not edits of it.
use serde_json::{Map, Value};
use xuan::{
    document::Point,
    i18n::tr,
    plugins::{
        jobs::Region,
        manifest::{InputKind, Surface},
    },
};

use super::{EditorApp, surfaces::SurfacePopup};

/// A box drawn with the AI Region tool.
#[derive(Clone, Debug)]
#[allow(dead_code)] // the box's popover (next) reads plugin and values
pub(super) struct AiBox {
    /// Document pixels; `fields` hold the action's region fields.
    pub region: Region,
    pub plugin: String,
    pub action: String,
    /// The action's other inputs.
    pub values: Map<String, Value>,
}

impl EditorApp {
    /// A new box with the first region action and its defaults.
    fn new_ai_box(&self, mut region: Region) -> Option<AiBox> {
        let (plugin, action, _) = self.surface_actions(Surface::Region).into_iter().next()?;
        let spec = self.plugins.manifest(&plugin)?.action(&action)?.clone();
        let regions = spec.regions_input()?;
        for field in &regions.fields {
            region.fields.insert(field.id.clone(), field.initial());
        }
        let values = (spec.inputs.iter())
            .filter(|i| i.kind != InputKind::Regions)
            .map(|i| (i.id.clone(), i.initial()))
            .collect();
        Some(AiBox {
            region,
            plugin,
            action,
            values,
        })
    }

    /// Add a box dragged from `start` to `end` (document pixels) and open
    /// its popover. Boxes under two pixels are ignored.
    pub(super) fn add_ai_box(&mut self, start: Point, end: Point) {
        let (x, y) = (start.x.min(end.x), start.y.min(end.y));
        let (width, height) = ((start.x - end.x).abs(), (start.y - end.y).abs());
        if width < 2.0 || height < 2.0 {
            return;
        }
        if let Some(ai_box) = self.new_ai_box(Region::rect(x, y, width, height)) {
            self.push_ai_box(ai_box);
        }
    }

    /// Add a box shaped like the selection, keeping its mask.
    pub(super) fn add_ai_box_from_selection(&mut self) {
        let Some(selection) = self.session().and_then(|s| s.document.selection.clone()) else {
            self.status = tr("Make a selection first").into();
            return;
        };
        if let Some(ai_box) = Region::from_mask(selection).and_then(|r| self.new_ai_box(r)) {
            self.push_ai_box(ai_box);
        }
    }

    fn push_ai_box(&mut self, ai_box: AiBox) {
        let Some(session) = self.session_mut() else {
            return;
        };
        session.ai_boxes.push(ai_box);
        let index = session.ai_boxes.len() - 1;
        session.ai_selected = Some(index);
        let document = session.document.id;
        self.open_surface_popup(SurfacePopup::Region { document, index });
    }

    /// Select the topmost box under `point` and open its popover.
    pub(super) fn select_ai_box_at(&mut self, point: Point) -> bool {
        let Some(session) = self.session_mut() else {
            return false;
        };
        let found = session
            .ai_boxes
            .iter()
            .rposition(|b| b.region.contains(point));
        session.ai_selected = found;
        let document = session.document.id;
        match found {
            Some(index) => self.open_surface_popup(SurfacePopup::Region { document, index }),
            None => self.surface_popup = None,
        }
        found.is_some()
    }

    pub(super) fn delete_ai_box(&mut self, index: usize) {
        if let Some(session) = self.session_mut()
            && index < session.ai_boxes.len()
        {
            session.ai_boxes.remove(index);
            session.ai_selected = None;
        }
        self.surface_popup = None;
    }

    pub(super) fn clear_ai_boxes(&mut self) {
        if let Some(session) = self.session_mut() {
            session.ai_boxes.clear();
            session.ai_selected = None;
        }
        self.surface_popup = None;
    }
}
