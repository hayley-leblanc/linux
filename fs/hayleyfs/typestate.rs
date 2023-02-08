/// Zero-sized types for persistence state
pub(crate) struct Dirty {}
pub(crate) struct InFlight {}
pub(crate) struct Clean {}

/// Zero-sized types for operation state
pub(crate) struct Start {}
pub(crate) struct Free {}
pub(crate) struct Alloc {}
pub(crate) struct Init {}
pub(crate) struct IncLink {}
pub(crate) struct Complete {}
pub(crate) struct Writeable {}
pub(crate) struct Written {}
pub(crate) struct IncSize {}

/// Traits to allow a transition from multiple legal typestates
pub(crate) trait Initialized {}
impl Initialized for Init {}
impl Initialized for Start {}

pub(crate) trait AddLink {}
impl AddLink for Alloc {}
impl AddLink for IncLink {}

pub(crate) trait CanWrite {}
impl CanWrite for Writeable {}
impl CanWrite for Init {}
