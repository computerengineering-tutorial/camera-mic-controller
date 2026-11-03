use std::process::Child;

/// RAII Guard for managing spawned child processes (FFmpeg / Pacat).
/// Ensures that when the struct is dropped or replaced, the previous child
/// is cleanly killed and waited upon, preventing orphaned or zombie processes.
pub struct ManagedChild(pub Option<Child>);

impl ManagedChild {
    pub fn new(child: Child) -> Self {
        Self(Some(child))
    }

    #[allow(dead_code)]
    pub fn empty() -> Self {
        Self(None)
    }

    pub fn replace(&mut self, new_child: Child) {
        self.kill_and_wait();
        self.0 = Some(new_child);
    }

    pub fn kill_and_wait(&mut self) {
        if let Some(mut ch) = self.0.take() {
            let _ = ch.kill();
            let _ = ch.wait();
        }
    }

    pub fn as_mut(&mut self) -> Option<&mut Child> {
        self.0.as_mut()
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
}
