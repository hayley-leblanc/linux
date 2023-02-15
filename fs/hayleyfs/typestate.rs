/// Zero-sized types for persistence state
#[derive(Debug)]
pub(crate) struct Dirty {}
#[derive(Debug)]
pub(crate) struct InFlight {}
#[derive(Debug)]
pub(crate) struct Clean {}

/// Zero-sized types for operation state
#[derive(Debug)]
pub(crate) struct Start {}
#[derive(Debug)]
pub(crate) struct Free {}
#[derive(Debug)]
pub(crate) struct Alloc {}
#[derive(Debug)]
pub(crate) struct Init {}
#[derive(Debug)]
pub(crate) struct IncLink {}
#[derive(Debug)]
pub(crate) struct Complete {}
#[derive(Debug)]
pub(crate) struct Writeable {}
#[derive(Debug)]
pub(crate) struct Written {}
#[derive(Debug)]
pub(crate) struct IncSize {}

/// Traits to allow a transition from multiple legal typestates
pub(crate) trait Initialized {}
impl Initialized for Init {}
impl Initialized for Start {}

pub(crate) trait AddLink {}
impl AddLink for Alloc {}
impl AddLink for IncLink {}
