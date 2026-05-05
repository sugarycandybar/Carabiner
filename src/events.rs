use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum ManagerEvent {
    StatusChanged(String),
    EndpointChanged { endpoint: String, claim_url: String },
    OutputReceived(String),
}

type Listener = Arc<dyn Fn(ManagerEvent) + Send + Sync + 'static>;

#[derive(Default)]
pub struct EventEmitter {
    listeners: Mutex<HashMap<&'static str, HashMap<u64, Listener>>>,
    listener_index: Mutex<HashMap<u64, &'static str>>,
    next_id: AtomicU64,
}

impl EventEmitter {
    pub fn connect<F>(&self, signal_name: &'static str, callback: F) -> u64
    where
        F: Fn(ManagerEvent) + Send + Sync + 'static,
    {
        let handler_id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.listeners
            .lock()
            .unwrap()
            .entry(signal_name)
            .or_default()
            .insert(handler_id, Arc::new(callback));
        self.listener_index
            .lock()
            .unwrap()
            .insert(handler_id, signal_name);
        handler_id
    }

    pub fn disconnect(&self, handler_id: u64) -> bool {
        let Some(signal_name) = self.listener_index.lock().unwrap().remove(&handler_id) else {
            return false;
        };

        let mut listeners = self.listeners.lock().unwrap();
        let Some(handlers) = listeners.get_mut(signal_name) else {
            return false;
        };
        let existed = handlers.remove(&handler_id).is_some();
        if handlers.is_empty() {
            listeners.remove(signal_name);
        }
        existed
    }

    pub fn emit(&self, signal_name: &'static str, event: ManagerEvent) {
        let callbacks = self
            .listeners
            .lock()
            .unwrap()
            .get(signal_name)
            .map(|handlers| handlers.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();

        for callback in callbacks {
            callback(event.clone());
        }
    }
}
