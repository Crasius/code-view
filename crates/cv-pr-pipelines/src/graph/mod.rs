//! The stage graph of a run, as Azure DevOps draws it: stage cards in columns with connectors,
//! on a canvas that pans and zooms with crisp text at every zoom.

pub(crate) mod layout;
pub(crate) mod pan_canvas;
pub(crate) mod sharp_text;
pub(crate) mod stage_card;
pub(crate) mod stage_graph;
pub(crate) mod zoom_controls;
