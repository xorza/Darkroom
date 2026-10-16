//! `Masters` — a [`lumos::CalibrationMasters`] bundle (master dark / flat /
//! bias / flat-dark + defect map) wrapped as a scenarium [`CustomValue`] so
//! a "build masters" node can hand it to a "stack lights" node on a wire.
//! `Display` reports which masters the bundle carries, and how many pixels its defect map names.

use std::any::Any;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::sync::Arc;

use lumos::CalibrationMasters;
use scenarium::{CustomValue, DataType, RamUsage, TypeId};

pub(crate) const MASTERS_TYPE_ID: TypeId = TypeId::literal("db1bc978-1d0b-4ffc-9a74-6220eff8908e");

pub(crate) const MASTERS_DATA_TYPE: DataType = DataType::Custom(MASTERS_TYPE_ID);

/// Calibration masters carried through the node graph.
#[derive(Debug)]
pub(crate) struct Masters {
    pub masters: CalibrationMasters,
}

impl CustomValue for Masters {
    fn type_id(&self) -> TypeId {
        MASTERS_TYPE_ID
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

impl Display for Masters {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut components = self.masters.components().peekable();
        if components.peek().is_none() {
            return f.write_str("no masters");
        }

        f.write_str("masters: ")?;
        for (index, component) in components.enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{component}")?;
        }
        if let Some(defects) = self.masters.defect_summary() {
            write!(f, "; {defects}")?;
        }
        Ok(())
    }
}

impl From<CalibrationMasters> for Masters {
    fn from(masters: CalibrationMasters) -> Self {
        Self { masters }
    }
}

#[cfg(test)]
mod tests {
    use common::CancelToken;
    use imaginarium::Buffer2;
    use lumos::{
        CalibrationMasters, CalibrationSet, CfaImage, CfaType, ImageMetadata, SigmaThreshold,
    };

    use crate::astro::masters::Masters;

    /// A bundle of one dark master and the defect map built from it.
    fn dark_bundle() -> CalibrationMasters {
        CalibrationMasters::from_images(
            CalibrationSet {
                dark: Some(CfaImage::from_plane(
                    Buffer2::new_filled(4, 4, 0.1),
                    CfaType::Mono,
                    ImageMetadata::default(),
                )),
                ..CalibrationSet::default()
            },
            SigmaThreshold::DEFAULT,
            &CancelToken::never(),
        )
        .unwrap()
    }

    #[test]
    fn display_empty_bundle() {
        assert_eq!(
            Masters::from(CalibrationMasters::default()).to_string(),
            "no masters"
        );
    }

    #[test]
    fn display_lists_present_masters() {
        assert_eq!(
            Masters::from(dark_bundle()).to_string(),
            "masters: dark, defects; 0 defective pixels, 0.000%"
        );
    }
}
