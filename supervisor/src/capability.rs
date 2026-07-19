//! Exact, sealed capability proofs for supervisor routes.
//!
//! Contract: observe, drive, and lifecycle are separate proof types. No proof
//! widens into another tier, and every privileged route accepts exactly one proof.
//! Positive drive state can only be projected from a held drive proof.

use std::collections::BTreeSet;
use std::io::{self, Write};

/// The three ordered policy tiers. Rust proof types remain exact: ordering does
/// not imply an automatic conversion to a broader or narrower proof.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CapabilityTier {
    /// Read-only session state and supervisor-owned observations.
    Observe,
    /// Input/control of an existing session.
    Drive,
    /// Process creation and termination.
    Lifecycle,
}

/// Auditable declaration of one capability-bearing route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RouteDeclaration {
    /// Stable route name used by tests and audit tooling.
    pub name: &'static str,
    /// The one exact capability tier accepted by this route.
    pub tier: CapabilityTier,
}

/// Complete W4 route inventory. Adding a capability-bearing surface requires an
/// entry here and a corresponding exact typed proof at the function boundary.
pub const ROUTE_DECLARATIONS: &[RouteDeclaration] = &[
    RouteDeclaration {
        name: "observe-session",
        tier: CapabilityTier::Observe,
    },
    RouteDeclaration {
        name: "observe-attestation",
        tier: CapabilityTier::Observe,
    },
    RouteDeclaration {
        name: "drive-presence",
        tier: CapabilityTier::Drive,
    },
    RouteDeclaration {
        name: "drive-input",
        tier: CapabilityTier::Drive,
    },
    RouteDeclaration {
        name: "lifecycle-spawn",
        tier: CapabilityTier::Lifecycle,
    },
];

/// Exact proof for read-only observation of a bounded session set.
#[derive(Debug)]
pub struct ObserveCapability {
    sessions: BTreeSet<String>,
}

/// Exact proof for driving a bounded session set.
#[derive(Debug)]
pub struct DriveCapability {
    sessions: BTreeSet<String>,
}

/// Lifecycle action authorized by a single-use grant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleAction {
    /// First launch of supervised sessions.
    Launch,
    /// Explicit relaunch after passive restore or exit.
    Relaunch,
}

impl LifecycleAction {
    /// Stable authorization-log representation.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Relaunch => "relaunch",
        }
    }
}

/// Exact proof for process lifecycle operations on a bounded session set.
#[derive(Debug)]
pub struct LifecycleCapability {
    action: LifecycleAction,
    sessions: BTreeSet<String>,
}

/// Read-only projection of sessions covered by an observe proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedSessions {
    sessions: BTreeSet<String>,
}

impl ObservedSessions {
    /// Returns whether this projection covers `session_id`.
    #[must_use]
    pub fn contains(&self, session_id: &str) -> bool {
        self.sessions.contains(session_id)
    }

    /// Number of observable sessions in this projection.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether no sessions are observable.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

/// Non-forgeable positive drive projection consumed by presentation code.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DrivePresence {
    sessions: BTreeSet<String>,
}

impl DrivePresence {
    /// A projection with no held drive capabilities.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Returns true only when a consumed drive grant covers `session_id`.
    #[must_use]
    pub fn is_driven(&self, session_id: &str) -> bool {
        self.sessions.contains(session_id)
    }
}

/// Issues the supervisor's read-only proof for an explicit session set.
///
/// Observation is the lowest tier; drive and lifecycle proofs are issued only by
/// the logged authorization flow in [`crate::attach`].
pub fn observe_sessions<I, S>(session_ids: I) -> io::Result<ObserveCapability>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    Ok(ObserveCapability {
        sessions: validated_sessions(session_ids)?,
    })
}

/// Projects a read-only view from an exact observe proof.
#[must_use]
pub fn observed_sessions(capability: &ObserveCapability) -> ObservedSessions {
    ObservedSessions {
        sessions: capability.sessions.clone(),
    }
}

impl DriveCapability {
    pub(crate) fn granted<I, S>(session_ids: I) -> io::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Ok(Self {
            sessions: validated_sessions(session_ids)?,
        })
    }

    /// Projects explicit agent-driving state from this held proof.
    #[must_use]
    pub fn presence(&self) -> DrivePresence {
        DrivePresence {
            sessions: self.sessions.clone(),
        }
    }

    fn permits(&self, session_id: &str) -> bool {
        self.sessions.contains(session_id)
    }
}

impl LifecycleCapability {
    pub(crate) fn granted<I, S>(action: LifecycleAction, session_ids: I) -> io::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Ok(Self {
            action,
            sessions: validated_sessions(session_ids)?,
        })
    }

    /// Authorized lifecycle action.
    #[must_use]
    pub fn action(&self) -> LifecycleAction {
        self.action
    }

    pub(crate) fn permits(&self, session_id: &str) -> bool {
        self.sessions.contains(session_id)
    }
}

/// Writes bytes to an existing PTY only under the exact drive proof.
///
/// The write fails before touching the PTY if the proof is not scoped to the
/// requested session. Write/flush failures are returned to the caller.
///
/// An observe proof cannot call this route:
///
/// ```compile_fail
/// use supervisor::capability::{observe_sessions, write_authorized_input};
/// let observe = observe_sessions(["session-000"]).unwrap();
/// let mut output = Vec::new();
/// write_authorized_input(&observe, "session-000", &mut output, b"pwd\n").unwrap();
/// ```
pub fn write_authorized_input(
    capability: &DriveCapability,
    session_id: &str,
    writer: &mut impl Write,
    bytes: &[u8],
) -> io::Result<()> {
    if !capability.permits(session_id) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "drive capability does not cover session",
        ));
    }
    writer.write_all(bytes)?;
    writer.flush()
}

fn validated_sessions<I, S>(session_ids: I) -> io::Result<BTreeSet<String>>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut sessions = BTreeSet::new();
    for session_id in session_ids {
        let session_id = session_id.into();
        validate_session_id(&session_id)?;
        if !sessions.insert(session_id) {
            return Err(invalid_input("duplicate capability session"));
        }
    }
    if sessions.is_empty() {
        return Err(invalid_input("capability session set cannot be empty"));
    }
    Ok(sessions)
}

fn validate_session_id(session_id: &str) -> io::Result<()> {
    if session_id.is_empty()
        || session_id.len() > 128
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(invalid_input("invalid capability session id"));
    }
    Ok(())
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
