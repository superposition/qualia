//! The default Thought Theater layout.

use rerun::blueprint::{
    Blueprint, Horizontal, Spatial2DView, Spatial3DView, Tabs, TextDocumentView, TimeSeriesView,
    Vertical,
};

use crate::connectome::ConnectomeEntityTaxonomy;
use crate::taxonomy::{ReplayEntityTaxonomy, WorldModelEntityTaxonomy};

/// Default viewer layout: proposal graph, accepted views, and the operational
/// slice side by side, each with its markdown companion below.
pub fn default_thought_theater_blueprint() -> Blueprint {
    let proposal_graph = Spatial2DView::new("Proposal Graph")
        .with_origin(WorldModelEntityTaxonomy::proposal_graph_root());
    let proposal_summary = TextDocumentView::new("Proposal Relations")
        .with_origin(WorldModelEntityTaxonomy::proposal_graph_summary_document())
        .with_contents(["$origin"]);
    let accepted_graph = Spatial2DView::new("Accepted Graph")
        .with_origin(WorldModelEntityTaxonomy::canonical_graph_root());
    let accepted_spatial = Spatial2DView::new("Accepted Spatial")
        .with_origin(WorldModelEntityTaxonomy::canonical_spatial_root());
    let accepted_summary = TextDocumentView::new("Accepted Relations")
        .with_origin(WorldModelEntityTaxonomy::canonical_graph_summary_document())
        .with_contents(["$origin"]);
    let runtime_spatial = Spatial2DView::new("Operational Slice")
        .with_origin(WorldModelEntityTaxonomy::operational_root());
    let runtime_summary = TextDocumentView::new("Operational State")
        .with_origin(WorldModelEntityTaxonomy::operational_summary())
        .with_contents(["$origin"]);
    let coach_summary = TextDocumentView::new("Coach Decisions")
        .with_origin(WorldModelEntityTaxonomy::coach_summary())
        .with_contents(["$origin"]);
    let replay_summary = TextDocumentView::new("Replay Timeline")
        .with_origin(ReplayEntityTaxonomy::summary())
        .with_contents(["$origin"]);

    let proposal_column = Vertical::new([proposal_graph.into(), proposal_summary.into()])
        .with_name("proposal_column")
        .with_row_shares([0.72, 0.28]);
    let accepted_views = Tabs::new([accepted_graph.into(), accepted_spatial.into()])
        .with_name("accepted_views");
    let accepted_column = Vertical::new([accepted_views.into(), accepted_summary.into()])
        .with_name("canonical_column")
        .with_row_shares([0.72, 0.28]);
    let runtime_tabs = Tabs::new([
        runtime_summary.into(),
        coach_summary.into(),
        replay_summary.into(),
    ])
    .with_name("runtime_tabs");
    let runtime_column = Vertical::new([runtime_spatial.into(), runtime_tabs.into()])
        .with_name("runtime_column")
        .with_row_shares([0.58, 0.42]);

    Blueprint::new(
        Horizontal::new([
            proposal_column.into(),
            accepted_column.into(),
            runtime_column.into(),
        ])
        .with_name("thought_theater_graph")
        .with_column_shares([1.0, 1.0, 0.7]),
    )
    .with_auto_layout(false)
    .with_auto_views(false)
}

/// Alias kept for callers that name the layout after the graph view.
pub fn default_graph_blueprint() -> Blueprint {
    default_thought_theater_blueprint()
}

/// The Brain layout: the point cloud in a 3D view beside the cloud's counts,
/// the current tick and the firing-count curve.
pub fn default_connectome_blueprint() -> Blueprint {
    let brain = Spatial3DView::new("Connectome")
        .with_origin(ConnectomeEntityTaxonomy::brain_root());
    let cloud = TextDocumentView::new("Cloud")
        .with_origin(ConnectomeEntityTaxonomy::cloud_summary())
        .with_contents(["$origin"]);
    let stream = TextDocumentView::new("Spike Stream")
        .with_origin(ConnectomeEntityTaxonomy::stream_summary())
        .with_contents(["$origin"]);
    let activity = TimeSeriesView::new("Firing per Tick")
        .with_origin(ConnectomeEntityTaxonomy::firing_count());

    let right = Vertical::new([cloud.into(), stream.into(), activity.into()])
        .with_name("connectome_stream")
        .with_row_shares([0.2, 0.2, 0.6]);
    Blueprint::new(
        Horizontal::new([brain.into(), right.into()])
            .with_name("floating_brain")
            .with_column_shares([0.74, 0.26]),
    )
    .with_auto_layout(false)
    .with_auto_views(false)
}
