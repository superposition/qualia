//! The device memory budget of the deployment target.
//!
//! T18 (#33): the costmap reduction's allocation plan must fit the 6 GiB the
//! Orin Nano lends it, leaving 2 GB of the 8 GB for the runtime. The plan is
//! pure arithmetic over the grid shape, so these tests need no device and run
//! on the default (no `cuda` feature) build.

use qualia_cuda::{
    memory_plan, CostmapStatsShape, ORIN_NANO_DEVICE_BUDGET_BYTES, VOXEL_D, VOXEL_W,
};

#[test]
fn fits_orin_nano() {
    // The largest grid the stack advertises is the world voxel footprint: the
    // planner reports `VOXEL_W` x `VOXEL_D` as its `max_grid_width` and
    // `max_grid_depth`, so that is the costmap shape the deployment must hold.
    let shape = CostmapStatsShape::new(VOXEL_W as u32, VOXEL_D as u32);
    let plan = memory_plan(&shape);
    assert!(
        plan <= ORIN_NANO_DEVICE_BUDGET_BYTES,
        "costmap plan for {}x{} is {plan} bytes, over the {ORIN_NANO_DEVICE_BUDGET_BYTES}-byte budget",
        shape.width,
        shape.depth
    );
}
