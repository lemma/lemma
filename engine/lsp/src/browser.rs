//! WASM entry point: run the Lemma LSP in the browser with JS-provided streams.
//!
//! The JS side (e.g. the playground) creates an AsyncIterator (into_server) and a
//! WritableStream (from_server), then calls `serve(ServerConfig::new(into_server, from_server))`.
//! The same LSP server as on desktop runs here; only the transport (streams) and
//! transport (streams) and entry point differ from native (stdio).

use futures::stream::TryStreamExt;
use tower_lsp_server::{LspService, Server};
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::stream::JsStream;

use crate::server;

/// Re-export engine WASM bindings (`Engine` in JS).
pub use lemma::wasm::WasmEngine;

#[wasm_bindgen]
pub struct ServerConfig {
    into_server: js_sys::AsyncIterator,
    from_server: web_sys::WritableStream,
}

#[wasm_bindgen]
impl ServerConfig {
    #[wasm_bindgen(constructor)]
    pub fn new(into_server: js_sys::AsyncIterator, from_server: web_sys::WritableStream) -> Self {
        Self {
            into_server,
            from_server,
        }
    }
}

/// Run the Lemma LSP over the given streams. Call from JS after creating
/// an AsyncIterator (client → server messages) and a WritableStream (server → client).
#[wasm_bindgen]
pub async fn serve(config: ServerConfig) -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    let ServerConfig {
        into_server,
        from_server,
    } = config;

    let input = JsStream::from(into_server);
    let input = input
        .map_ok(|value| {
            value
                .dyn_into::<js_sys::Uint8Array>()
                .expect("stream item must be Uint8Array")
                .to_vec()
        })
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))
        .into_async_read();

    let output = JsCast::unchecked_into::<wasm_streams::writable::sys::WritableStream>(from_server);
    let output = wasm_streams::WritableStream::from_raw(output);
    let output = output.try_into_async_write().map_err(|err| err.0)?;

    let (service, messages) = LspService::new(server::LemmaLanguageServer::new);
    Server::new(input, output, messages).serve(service).await;

    Ok(())
}
