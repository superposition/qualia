//! Attach to the shared-memory arena, creating it when this host is the owner.
//!
//! In production the supervisor creates the region and every runner opens it.
//! `autocreate` exists for a host run and for tests; a region this process
//! creates is owned by [`CREATED`] so it outlives the handler that asked for it
//! and is unmapped with the process.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use qualia_shm::ShmRegion;

/// Regions this process created, keyed by name, so a second handler for the
/// same name attaches instead of recreating it.
static CREATED: LazyLock<Mutex<HashMap<String, ShmRegion>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Open `name`, optionally creating it when it does not exist yet.
pub fn open_region(name: &str, autocreate: bool) -> Result<ShmRegion, String> {
    match ShmRegion::open(name) {
        Ok(region) => Ok(region),
        Err(open_error) => {
            if !autocreate {
                return Err(format!("cannot open {name}: {open_error}"));
            }
            let mut created = CREATED.lock().expect("created-region lock");
            if !created.contains_key(name) {
                match ShmRegion::create(name) {
                    Ok(region) => {
                        created.insert(name.to_string(), region);
                    }
                    Err(create_error) => {
                        return Err(format!("cannot create {name}: {create_error}"));
                    }
                }
            }
            drop(created);
            ShmRegion::open(name).map_err(|error| format!("cannot open {name}: {error}"))
        }
    }
}
