/// Zero-sized types for persistence state
pub(crate) struct Dirty {}
pub(crate) struct InFlight {}
pub(crate) struct Clean {}

// Zero-sized types for operation state
// pub(crate) struct Start {}
// pub(crate) struct Free {}
// pub(crate) struct AllocStarted {}
pub(crate) struct Alloc {}
// pub(crate) struct Init {}
