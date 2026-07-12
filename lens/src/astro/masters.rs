//! `Masters` — a [`lumos::CalibrationMasters`] bundle (master dark / flat /
//! bias / flat-dark + defect map) wrapped as a scenarium [`CustomValue`] so
//! a "build masters" node can hand it to a "stack lights" node on a wire.
//! `Display` reports which masters the bundle carries.

use std::any::Any;
use std::sync::{Arc, LazyLock};

use lumos::CalibrationMasters;
use scenarium::data::{CustomValue, DataType, RamUsage, TypeId};

pub static MASTERS_TYPE_ID: LazyLock<TypeId> =
    LazyLock::new(|| "db1bc978-1d0b-4ffc-9a74-6220eff8908e".into());

pub static MASTERS_DATA_TYPE: LazyLock<DataType> =
    LazyLock::new(|| DataType::Custom(*MASTERS_TYPE_ID));

/// Calibration masters carried through the node graph.
#[derive(Debug)]
pub struct Masters {
    pub masters: CalibrationMasters,
}

impl Masters {
    pub fn new(masters: CalibrationMasters) -> Self {
        Self { masters }
    }
}

impl CustomValue for Masters {
    fn type_id(&self) -> TypeId {
        *MASTERS_TYPE_ID
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }

    fn ram_bytes(&self) -> RamUsage {
        // Calibration frames are CPU-only (lumos has no GPU backend).
        RamUsage {
            cpu: self.masters.ram_bytes(),
            gpu: 0,
        }
    }
}

impl std::fmt::Display for Masters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let m = &self.masters;
        let mut parts = Vec::new();
        if m.master_dark.is_some() {
            parts.push("dark");
        }
        if m.master_flat.is_some() {
            parts.push("flat");
        }
        if m.master_bias.is_some() {
            parts.push("bias");
        }
        if m.master_flat_dark.is_some() {
            parts.push("flat-dark");
        }
        if m.defect_map.is_some() {
            parts.push("defects");
        }
        if parts.is_empty() {
            write!(f, "no masters")
        } else {
            write!(f, "masters: {}", parts.join(", "))
        }
    }
}

impl From<CalibrationMasters> for Masters {
    fn from(masters: CalibrationMasters) -> Self {
        Masters::new(masters)
    }
}

#[cfg(test)]
mod tests {
    use lumos::DefectMap;

    use super::*;

    fn bundle(defects: bool) -> CalibrationMasters {
        CalibrationMasters {
            master_dark: None,
            master_flat: None,
            master_bias: None,
            master_flat_dark: None,
            defect_map: defects.then(DefectMap::default),
        }
    }

    #[test]
    fn display_empty_bundle() {
        assert_eq!(Masters::new(bundle(false)).to_string(), "no masters");
    }

    #[test]
    fn display_lists_present_masters() {
        assert_eq!(Masters::new(bundle(true)).to_string(), "masters: defects");
    }
}
