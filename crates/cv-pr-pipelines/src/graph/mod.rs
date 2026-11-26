//! The stage graph of a run, as Azure DevOps draws it: stage cards in columns with connectors,
//! on a canvas that pans and zooms with crisp text at every zoom.

pub mod layout;
pub mod pan_canvas;
pub mod sharp_text;
pub mod stage_card;
pub mod stage_graph;
pub mod zoom_controls;
