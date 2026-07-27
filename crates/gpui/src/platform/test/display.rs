use crate::{Bounds, DisplayId, Pixels, PlatformDisplay, Point, px};
use anyhow::{Ok, Result};

/// A fake display, used for testing.
#[derive(Debug)]
pub struct TestDisplay {
    id: DisplayId,
    uuid: uuid::Uuid,
    name: Option<String>,
    bounds: Bounds<Pixels>,
}

impl TestDisplay {
    pub(crate) fn new() -> Self {
        Self::new_with(DisplayId(1), None, uuid::Uuid::new_v4())
    }

    /// Creates a display reporting the given id, human-readable name, and uuid.
    ///
    /// Two displays may deliberately share a name — that is the duplicate-hardware case
    /// real multi-monitor code has to disambiguate by uuid — but their ids must differ,
    /// since a window identifies the display it is on by id.
    pub fn new_with(id: DisplayId, name: Option<String>, uuid: uuid::Uuid) -> Self {
        TestDisplay {
            id,
            uuid,
            name,
            bounds: Bounds::from_corners(Point::default(), Point::new(px(1920.), px(1080.))),
        }
    }
}

impl PlatformDisplay for TestDisplay {
    fn id(&self) -> crate::DisplayId {
        self.id
    }

    fn uuid(&self) -> Result<uuid::Uuid> {
        Ok(self.uuid)
    }

    fn name(&self) -> Option<String> {
        self.name.clone()
    }

    fn bounds(&self) -> crate::Bounds<crate::Pixels> {
        self.bounds
    }
}
