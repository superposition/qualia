//! The `qualia-l3-belief` process: hand layer 3 to the compiled-in backend.
//!
//! Nothing else belongs in this binary. The belief loop, the seeds it writes
//! into the layer slot and every line it logs belong to `qualia-metal` or
//! `qualia-cuda`; a fragment of that loop living here would put a second writer
//! on the same layer slot.

fn main() {
    qualia_l3_belief::run();
}
