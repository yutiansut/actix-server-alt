use http::header::HeaderMap;

use crate::util::date::Date;

/// Context is connection specific struct contain states for processing.
/// It needs manually reset with every new successfully decoded request.
/// See `Context::reset` method for detail.
pub(super) struct Context<'a> {
    state: ContextState,
    ctype: ConnectionType,
    /// header map reused by next request.
    pub(super) header_cache: Option<HeaderMap>,
    /// smart pointer of cached date with 500 milli second update interval.
    pub(super) date: &'a Date,
}

impl<'a> Context<'a> {
    pub(super) fn new(date: &'a Date) -> Self {
        Self {
            state: ContextState::new(),
            ctype: ConnectionType::Init,
            header_cache: None,
            date,
        }
    }

    #[inline(always)]
    pub(super) fn is_expect_header(&self) -> bool {
        self.state.contains(ContextState::EXPECT)
    }

    #[inline(always)]
    pub(super) fn is_connect_method(&self) -> bool {
        self.state.contains(ContextState::CONNECT)
    }

    #[inline(always)]
    pub(super) fn is_force_close(&self) -> bool {
        self.state.contains(ContextState::FORCE_CLOSE)
    }

    /// Context should be reset when a new request is decoded.
    ///
    /// A reset of context only happen on a keep alive connection type.
    #[inline(always)]
    pub(super) fn reset(&mut self) {
        self.ctype = ConnectionType::KeepAlive;
        self.state = ContextState::new();
    }

    pub(super) fn set_expect_header(&mut self) {
        self.state.insert(ContextState::EXPECT)
    }

    pub(super) fn set_connect_method(&mut self) {
        self.state.insert(ContextState::CONNECT)
    }

    pub(super) fn set_force_close(&mut self) {
        self.state.insert(ContextState::FORCE_CLOSE)
    }

    #[inline(always)]
    pub(super) fn set_ctype(&mut self, ctype: ConnectionType) {
        self.ctype = ctype;
    }

    #[inline(always)]
    pub(super) fn ctype(&self) -> ConnectionType {
        self.ctype
    }
}

/// A set of state for current request that are used after request's ownership is passed
/// to service call.
struct ContextState(u8);

impl ContextState {
    /// Enable when current request has 100-continue header.
    const EXPECT: Self = Self(0b_0001);

    /// Enable when current request is CONNECT method.
    const CONNECT: Self = Self(0b_0010);

    /// Want a force close after current request served.
    ///
    /// This is for situation like partial read of request body. Which could leave artifact
    /// unread data in connection that can interfere with next request(If the connection is kept
    /// alive).
    const FORCE_CLOSE: Self = Self(0b_0100);

    #[inline(always)]
    const fn new() -> Self {
        Self(0)
    }

    #[inline(always)]
    fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    #[inline(always)]
    const fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

/// Represents various types of connection
#[derive(Copy, Clone, PartialEq, Debug)]
pub(super) enum ConnectionType {
    /// A connection that has no request yet.
    Init,

    /// Close connection after response
    Close,

    /// Keep connection alive after response
    KeepAlive,

    /// Connection is upgraded to different type
    Upgrade,
}
