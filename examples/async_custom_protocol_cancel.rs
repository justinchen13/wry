// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Responds to an asynchronous custom protocol from worker threads while the
//! page keeps cancelling the in-flight requests (aborted `fetch` calls and
//! page reloads), so WebKit stops tasks that are about to be answered.
//!
//! The page reloads `RELOADS` times, then loads once more without cancelling
//! anything and checks that every response arrives. The process exits with 0
//! when that check passes, and with 1 on failure or after a timeout.
//!
//! ```sh
//! cargo run --example async_custom_protocol_cancel
//! # with an optional reload count:
//! cargo run --example async_custom_protocol_cancel -- 500
//! ```

use std::{
  borrow::Cow,
  sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
  },
  thread,
  time::{Duration, Instant},
};

use tao::{
  event::{Event, WindowEvent},
  event_loop::{ControlFlow, EventLoopBuilder},
  window::WindowBuilder,
};
use wry::{
  WebViewBuilder,
  http::{Response, header::CONTENT_TYPE},
};

/// Requests sent by each page load.
const REQUESTS: usize = 64;
const DEFAULT_RELOADS: usize = 300;
const TIMEOUT: Duration = Duration::from_secs(120);

enum UserEvent {
  Exit(i32),
}

fn stress_page() -> String {
  format!(
    r#"<!doctype html><script>
for (let i = 0; i < {REQUESTS}; i++) {{
  const controller = new AbortController();
  fetch(`wry://localhost/data/${{i}}`, {{ signal: controller.signal }}).catch(() => {{}});
  if (i % 2) setTimeout(() => controller.abort(), Math.random() * 5);
}}
setTimeout(() => location.reload(), 5 + Math.random() * 20);
</script>"#
  )
}

fn final_page() -> String {
  format!(
    r#"<!doctype html><script>
const requests = [];
for (let i = 0; i < {REQUESTS}; i++) {{
  requests.push(fetch(`wry://localhost/data/${{i}}`).then((r) => r.text()));
}}
Promise.allSettled(requests).then((results) => {{
  const ok = results.filter((r) => r.status === 'fulfilled' && r.value.length === 4096).length;
  window.ipc.postMessage(`${{ok}}/{REQUESTS}`);
}});
</script>"#
  )
}

fn main() -> wry::Result<()> {
  let reloads: usize = std::env::args()
    .nth(1)
    .and_then(|n| n.parse().ok())
    .unwrap_or(DEFAULT_RELOADS);

  let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
  let proxy = event_loop.create_proxy();
  let window = WindowBuilder::new().build(&event_loop).unwrap();

  let page_loads = Arc::new(AtomicUsize::new(0));
  let start = Instant::now();

  let timeout_proxy = proxy.clone();
  let timeout_loads = page_loads.clone();
  thread::spawn(move || {
    thread::sleep(TIMEOUT);
    eprintln!(
      "timeout after {} page loads",
      timeout_loads.load(Ordering::SeqCst)
    );
    let _ = timeout_proxy.send_event(UserEvent::Exit(1));
  });

  let ipc_proxy = proxy.clone();
  let builder = WebViewBuilder::new()
    .with_asynchronous_custom_protocol("wry".into(), move |_webview_id, request, responder| {
      let path = request.uri().path().to_string();
      let body: Cow<'static, [u8]> = if path == "/" {
        let n = page_loads.fetch_add(1, Ordering::SeqCst);
        if n < reloads {
          stress_page().into_bytes().into()
        } else {
          final_page().into_bytes().into()
        }
      } else {
        vec![b'x'; 4096].into()
      };
      let content_type = if path == "/" {
        "text/html"
      } else {
        "text/plain"
      };

      // Answer from a worker thread, a little later, as an async runtime would.
      thread::spawn(move || {
        thread::sleep(Duration::from_millis(body.len() as u64 % 3));
        responder.respond(
          Response::builder()
            .header(CONTENT_TYPE, content_type)
            .body(body)
            .unwrap(),
        );
      });
    })
    .with_ipc_handler(move |request| {
      let result = request.body();
      let ok = result == &format!("{REQUESTS}/{REQUESTS}");
      println!(
        "final page load: {result} responses received after {reloads} reloads in {:.1?}",
        start.elapsed()
      );
      let _ = ipc_proxy.send_event(UserEvent::Exit(if ok { 0 } else { 1 }));
    })
    .with_url("wry://localhost");

  #[cfg(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
  ))]
  let _webview = builder.build(&window)?;
  #[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
  )))]
  let _webview = {
    use tao::platform::unix::WindowExtUnix;
    use wry::WebViewBuilderExtUnix;
    let vbox = window.default_vbox().unwrap();
    builder.build_gtk(vbox)?
  };

  event_loop.run(move |event, _, control_flow| {
    *control_flow = ControlFlow::Wait;

    match event {
      Event::UserEvent(UserEvent::Exit(code)) => std::process::exit(code),
      Event::WindowEvent {
        event: WindowEvent::CloseRequested,
        ..
      } => *control_flow = ControlFlow::Exit,
      _ => {}
    }
  });
}
