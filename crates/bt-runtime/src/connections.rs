use bt_core::{BelltowerConfig, ConnectionDescriptor, ConnectionId};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct ConnectionRegistry {
    connections: BTreeMap<ConnectionId, ConnectionDescriptor>,
}

impl ConnectionRegistry {
    #[must_use]
    pub fn from_config(config: &BelltowerConfig) -> Self {
        Self {
            connections: config
                .connections
                .iter()
                .cloned()
                .map(|descriptor| (descriptor.id.clone(), descriptor))
                .collect(),
        }
    }

    #[must_use]
    pub fn all(&self) -> Vec<ConnectionDescriptor> {
        self.connections.values().cloned().collect()
    }

    #[must_use]
    pub fn get(&self, id: &ConnectionId) -> Option<ConnectionDescriptor> {
        self.connections.get(id).cloned()
    }
}
