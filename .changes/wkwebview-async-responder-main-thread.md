---
"wry": patch
---

On macOS and iOS, deliver custom protocol responses on the main thread when the responder is called from another thread. Fixes a process abort when a task was stopped (for example by a reload or an aborted request) while the response was being delivered, and stops the responding thread from blocking until the main thread is free.
