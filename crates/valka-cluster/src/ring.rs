use hashring::HashRing as HRing;

/// Consistent hash ring for partition-to-node mapping
pub struct HashRing {
    inner: HRing<String>,
}

impl HashRing {
    pub fn new() -> Self {
        Self {
            inner: HRing::new(),
        }
    }

    pub fn add_node(&mut self, node_id: &str) {
        // Add virtual nodes for better distribution
        for i in 0..64 {
            self.inner.add(format!("{node_id}#vn{i}"));
        }
    }

    pub fn remove_node(&mut self, node_id: &str) {
        for i in 0..64 {
            self.inner.remove(&format!("{node_id}#vn{i}"));
        }
    }

    pub fn get_node(&self, key: &str) -> Option<String> {
        self.inner.get(&key.to_string()).map(|s| {
            // Strip virtual node suffix
            s.split('#').next().unwrap_or(s).to_string()
        })
    }
}

impl Default for HashRing {
    fn default() -> Self {
        Self::new()
    }
}
