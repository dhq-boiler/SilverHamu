use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::{io, net::SocketAddr};

pub(super) type Ids = BTreeSet<String>;
pub(super) type Routes = BTreeMap<String, [SocketAddr; 2]>;

pub(super) fn valid_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

pub(super) fn endpoints(entries: &Value, remote: bool) -> io::Result<Routes> {
    let entries = entries
        .as_array()
        .filter(|a| matches!(a.len(), 3 | 5))
        .ok_or_else(|| io::Error::other("configure three or five members"))?;
    let mut routes = Routes::new();
    for member in entries {
        let id = member["id"]
            .as_str()
            .filter(|id| valid_id(id))
            .ok_or_else(|| io::Error::other("invalid member id"))?;
        let address = |field: &str| {
            member[field]
                .as_str()
                .unwrap_or("")
                .parse::<SocketAddr>()
                .map_err(io::Error::other)
        };
        if routes
            .insert(id.into(), [address("peer")?, address("client")?])
            .is_some()
        {
            return Err(io::Error::other("duplicate member id"));
        }
    }
    validate_routes(&routes, remote)?;
    Ok(routes)
}

pub(super) fn validate_routes(routes: &Routes, remote: bool) -> io::Result<()> {
    let mut addresses = BTreeSet::new();
    for (id, pair) in routes {
        for addr in pair {
            if !valid_id(id)
                || addr.port() == 0
                || addr.ip().is_unspecified()
                || addr.ip().is_multicast()
                || (!remote && !addr.ip().is_loopback())
                || !addresses.insert(*addr)
            {
                return Err(io::Error::other("invalid or duplicate cluster endpoint"));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Membership {
    pub phase: Phase,
    pub old: Ids,
    pub target: Ids,
    pub routes: Routes,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Stable,
    CatchUp,
    Joint,
}

impl Membership {
    pub fn bootstrap(ids: Ids) -> Self {
        Self {
            phase: Phase::Stable,
            old: ids.clone(),
            target: ids,
            routes: Routes::new(),
        }
    }
    pub fn phase_name(&self) -> &'static str {
        match self.phase {
            Phase::Stable => "stable",
            Phase::CatchUp => "catch_up",
            Phase::Joint => "joint",
        }
    }
    pub fn command(&self) -> Value {
        json!({"membership":{"phase":self.phase_name(),"old":self.old,"target":self.target,"routes":self.routes}})
    }
    pub fn parse(command: &Value) -> io::Result<Self> {
        let v = &command["membership"];
        if command.as_object().is_none_or(|o| o.len() != 1)
            || v.as_object().is_none_or(|o| o.len() != 4)
        {
            return Err(io::Error::other("invalid membership command"));
        }
        let phase = match v["phase"].as_str() {
            Some("stable") => Phase::Stable,
            Some("catch_up") => Phase::CatchUp,
            Some("joint") => Phase::Joint,
            _ => return Err(io::Error::other("invalid membership phase")),
        };
        let ids = |a: &Value| -> io::Result<Ids> {
            let list: Vec<String> = serde_json::from_value(a.clone())?;
            let ids: Ids = list.iter().cloned().collect();
            if !matches!(list.len(), 3 | 5)
                || ids.len() != list.len()
                || !ids.iter().all(|id| valid_id(id))
            {
                return Err(io::Error::other("invalid voters"));
            }
            Ok(ids)
        };
        let old = ids(&v["old"])?;
        let target = ids(&v["target"])?;
        let routes: Routes = serde_json::from_value(v["routes"].clone())?;
        if routes.keys().cloned().collect::<Ids>() != old.union(&target).cloned().collect() {
            return Err(io::Error::other("incomplete membership routes"));
        }
        validate_routes(&routes, true)?;
        Ok(Self {
            phase,
            old,
            target,
            routes,
        })
    }
    pub fn voter(&self, id: &str) -> bool {
        match self.phase {
            Phase::Stable => self.target.contains(id),
            Phase::CatchUp => self.old.contains(id),
            Phase::Joint => self.old.contains(id) || self.target.contains(id),
        }
    }
    /// The two majorities must each reach the index; overlapping members count in both.
    pub fn quorum_index(&self, matched: impl Fn(&str) -> u64) -> u64 {
        let majority = |set: &Ids| {
            let mut indices: Vec<_> = set.iter().map(|id| matched(id)).collect();
            indices.sort_unstable();
            indices[(indices.len() - 1) / 2]
        };
        match self.phase {
            Phase::Stable => majority(&self.target),
            Phase::CatchUp => majority(&self.old),
            Phase::Joint => majority(&self.old).min(majority(&self.target)),
        }
    }
}
