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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::thread;

    #[test]
    fn test_connect_and_emit() {
        let emitter = EventEmitter::default();
        let received = Arc::new(Mutex::new(Vec::new()));

        let received_clone = received.clone();
        let handler_id = emitter.connect("status-changed", move |event| {
            if let ManagerEvent::StatusChanged(status) = event {
                received_clone.lock().unwrap().push(status);
            }
        });

        assert_eq!(handler_id, 1);

        emitter.emit(
            "status-changed",
            ManagerEvent::StatusChanged("Running".to_string()),
        );
        emitter.emit(
            "status-changed",
            ManagerEvent::StatusChanged("Stopped".to_string()),
        );
        emitter.emit(
            "other-event",
            ManagerEvent::StatusChanged("Ignored".to_string()),
        );

        let results = received.lock().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], "Running");
        assert_eq!(results[1], "Stopped");
    }

    #[test]
    fn test_disconnect() {
        let emitter = EventEmitter::default();
        let counter = Arc::new(Mutex::new(0));

        let counter_clone = counter.clone();
        let id = emitter.connect("test", move |_| {
            *counter_clone.lock().unwrap() += 1;
        });

        emitter.emit("test", ManagerEvent::OutputReceived("hello".to_string()));
        assert_eq!(*counter.lock().unwrap(), 1);

        assert!(emitter.disconnect(id));
        assert!(!emitter.disconnect(id));

        emitter.emit("test", ManagerEvent::OutputReceived("world".to_string()));
        assert_eq!(*counter.lock().unwrap(), 1);
    }

    #[test]
    fn test_multiple_listeners() {
        let emitter = EventEmitter::default();
        let log = Arc::new(Mutex::new(Vec::new()));

        let log1 = log.clone();
        emitter.connect("event", move |_| {
            log1.lock().unwrap().push(1);
        });

        let log2 = log.clone();
        emitter.connect("event", move |_| {
            log2.lock().unwrap().push(2);
        });

        emitter.emit("event", ManagerEvent::OutputReceived("ping".to_string()));

        let results = log.lock().unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.contains(&1));
        assert!(results.contains(&2));
    }

    #[test]
    fn test_thread_safety() {
        let emitter = Arc::new(EventEmitter::default());
        let counter = Arc::new(Mutex::new(0));

        let mut handles = Vec::new();
        for _ in 0..10 {
            let emitter_clone = emitter.clone();
            let counter_clone = counter.clone();
            handles.push(thread::spawn(move || {
                let id = emitter_clone.connect("thread-event", move |_| {
                    *counter_clone.lock().unwrap() += 1;
                });
                emitter_clone.emit(
                    "thread-event",
                    ManagerEvent::OutputReceived("ping".to_string()),
                );
                emitter_clone.disconnect(id);
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }
    }
}
