/**
 * LSP client for the Lemma playground.
 *
 * Handles LSP JSON-RPC protocol framing (Content-Length headers),
 * stream plumbing for the in-process LSP server, and Monaco marker updates.
 */

import { serve, ServerConfig } from './lsp.js';

// ---------------------------------------------------------------------------
// Async channel: a simple queue that backs an AsyncIterator for the server.
// ---------------------------------------------------------------------------

/**
 * Queue-backed channel that implements AsyncIterable (Symbol.asyncIterator)
 * so Rust js_sys::AsyncIterator can consume it. Yields Uint8Array chunks.
 */
function createChannel() {
    const queue = [];
    let waiting = null;
    let closed = false;

    const channel = {
        send(data) {
            if (closed) return;
            if (waiting) {
                const resolve = waiting;
                waiting = null;
                resolve({ value: data, done: false });
            } else {
                queue.push(data);
            }
        },
        close() {
            closed = true;
            if (waiting) {
                const resolve = waiting;
                waiting = null;
                resolve({ value: undefined, done: true });
            }
        },
        next() {
            if (queue.length > 0) {
                return Promise.resolve({ value: queue.shift(), done: false });
            }
            if (closed) {
                return Promise.resolve({ value: undefined, done: true });
            }
            return new Promise(function (resolve) { waiting = resolve; });
        },
    };
    channel[Symbol.asyncIterator] = function () { return this; };
    return channel;
}

// ---------------------------------------------------------------------------
// LSP protocol framing
// ---------------------------------------------------------------------------

const HEADER_SEPARATOR = '\r\n\r\n';
const CONTENT_LENGTH_RE = /Content-Length:\s*(\d+)/i;
const encoder = new TextEncoder();
const decoder = new TextDecoder();

function encodeMessage(json) {
    const body = JSON.stringify(json);
    const bodyBytes = encoder.encode(body);
    const header = 'Content-Length: ' + bodyBytes.byteLength + HEADER_SEPARATOR;
    const headerBytes = encoder.encode(header);
    const frame = new Uint8Array(headerBytes.byteLength + bodyBytes.byteLength);
    frame.set(headerBytes, 0);
    frame.set(bodyBytes, headerBytes.byteLength);
    return frame;
}

// ---------------------------------------------------------------------------
// LSP Client
// ---------------------------------------------------------------------------

export class LspClient {
    /**
     * @param {object} monacoInstance  The `monaco` global (for setModelMarkers)
     */
    constructor(monacoInstance) {
        this._monaco = monacoInstance;
        this._nextId = 1;
        this._pending = new Map();
        this._diagnosticsCallback = null;
        this._channel = null;
        this._receiveBuffer = new Uint8Array(0);
        this._running = false;
    }

    /**
     * Launch the LSP server. Uses bundled serve/ServerConfig unless overridden.
     * @param {Function} [serveFn]  Optional; defaults to bundled LSP serve
     * @param {Function} [ServerConfigCls]  Optional; defaults to bundled ServerConfig
     */
    async start(serveFn, ServerConfigCls) {
        const runServe = serveFn ?? serve;
        const RunConfig = ServerConfigCls ?? ServerConfig;

        this._channel = createChannel();

        const self = this;

        const serverToClient = new WritableStream({
            write(chunk) {
                if (chunk instanceof Uint8Array) {
                    self._onServerBytes(chunk);
                }
            },
        });

        const config = new RunConfig(this._channel, serverToClient);
        this._running = true;

        runServe(config).then(function () {
            self._running = false;
        }).catch(function (err) {
            console.error('LSP server stopped:', err);
            self._running = false;
        });
    }

    /**
     * Send the LSP initialize request and the initialized notification.
     */
    async initialize() {
        await this.sendRequest('initialize', {
            processId: null,
            capabilities: {},
            rootUri: null,
        });
        this.sendNotification('initialized', {});
    }

    /**
     * Notify the server that a file was opened.
     */
    didOpen(uri, languageId, version, text) {
        this.sendNotification('textDocument/didOpen', {
            textDocument: { uri: uri, languageId: languageId, version: version, text: text },
        });
    }

    /**
     * Notify the server that a file changed (full sync).
     */
    didChange(uri, version, text) {
        this.sendNotification('textDocument/didChange', {
            textDocument: { uri: uri, version: version },
            contentChanges: [{ text: text }],
        });
    }

    /**
     * Notify the server that a file was closed.
     */
    didClose(uri) {
        this.sendNotification('textDocument/didClose', {
            textDocument: { uri: uri },
        });
    }

    /**
     * Request textDocument/formatting from the server.
     * Returns an array of TextEdit.
     */
    async formatting(uri, tabSize, insertSpaces) {
        return this.sendRequest('textDocument/formatting', {
            textDocument: { uri: uri },
            options: { tabSize: tabSize, insertSpaces: insertSpaces },
        });
    }

    /**
     * Request textDocument/semanticTokens/full from the server.
     * Returns { data: number[] } or null.
     */
    async semanticTokensFull(uri) {
        return this.sendRequest('textDocument/semanticTokens/full', {
            textDocument: { uri: uri },
        });
    }

    /**
     * Request textDocument/completion from the server.
     * Returns { items: CompletionItem[] } or CompletionItem[].
     */
    async completion(uri, line, character) {
        return this.sendRequest('textDocument/completion', {
            textDocument: { uri: uri },
            position: { line: line, character: character },
        });
    }

    /**
     * Register a callback for textDocument/publishDiagnostics notifications.
     * @param {Function} callback  Receives (uri, diagnostics)
     */
    onDiagnostics(callback) {
        this._diagnosticsCallback = callback;
    }

    /**
     * Send a JSON-RPC request and return a Promise for the result.
     */
    sendRequest(method, params) {
        const id = this._nextId++;
        const msg = { jsonrpc: '2.0', id: id, method: method, params: params };
        this._send(msg);
        const pending = this._pending;
        return new Promise(function (resolve, reject) {
            pending.set(id, { resolve: resolve, reject: reject });
        });
    }

    /**
     * Send a JSON-RPC notification (no response expected).
     */
    sendNotification(method, params) {
        const msg = { jsonrpc: '2.0', method: method, params: params };
        this._send(msg);
    }

    /**
     * Stop the LSP server by closing the input channel.
     */
    stop() {
        if (this._channel) {
            this._channel.close();
        }
        this._running = false;
    }

    // -- internals --

    _send(json) {
        if (!this._channel) return;
        this._channel.send(encodeMessage(json));
    }

    _onServerBytes(chunk) {
        const merged = new Uint8Array(this._receiveBuffer.byteLength + chunk.byteLength);
        merged.set(this._receiveBuffer, 0);
        merged.set(chunk, this._receiveBuffer.byteLength);
        this._receiveBuffer = merged;

        this._drainFrames();
    }

    _drainFrames() {
        while (true) {
            const buf = this._receiveBuffer;
            const text = decoder.decode(buf);

            const sepIndex = text.indexOf(HEADER_SEPARATOR);
            if (sepIndex === -1) break;

            const headerPart = text.substring(0, sepIndex);
            const match = CONTENT_LENGTH_RE.exec(headerPart);
            if (!match) {
                console.error('LSP: missing Content-Length in header:', headerPart);
                break;
            }
            const contentLength = parseInt(match[1], 10);

            const headerByteLength = encoder.encode(headerPart + HEADER_SEPARATOR).byteLength;
            const totalNeeded = headerByteLength + contentLength;

            if (buf.byteLength < totalNeeded) break;

            const bodyBytes = buf.slice(headerByteLength, totalNeeded);
            this._receiveBuffer = buf.slice(totalNeeded);

            const bodyText = decoder.decode(bodyBytes);
            let json;
            try {
                json = JSON.parse(bodyText);
            } catch (e) {
                console.error('LSP: failed to parse JSON body:', bodyText, e);
                continue;
            }

            this._handleMessage(json);
        }
    }

    _handleMessage(msg) {
        if (msg.id !== undefined && msg.id !== null && this._pending.has(msg.id)) {
            const entry = this._pending.get(msg.id);
            this._pending.delete(msg.id);
            if (msg.error) {
                entry.reject(msg.error);
            } else {
                entry.resolve(msg.result);
            }
            return;
        }

        if (msg.method === 'textDocument/publishDiagnostics' && msg.params) {
            if (this._diagnosticsCallback) {
                this._diagnosticsCallback(msg.params.uri, msg.params.diagnostics || []);
            }
            return;
        }
    }

    // -- Monaco integration --

    /**
     * Convert LSP diagnostics to Monaco markers and set them on the given model.
     *
     * @param {object} model       Monaco editor model
     * @param {Array}  diagnostics LSP Diagnostic[]
     */
    setMonacoMarkers(model, diagnostics) {
        const monaco = this._monaco;
        if (!monaco) return;

        const markers = diagnostics.map(function (d) {
            return {
                startLineNumber: d.range.start.line + 1,
                startColumn: d.range.start.character + 1,
                endLineNumber: d.range.end.line + 1,
                endColumn: d.range.end.character + 1,
                message: d.message,
                severity: lspSeverityToMonaco(monaco, d.severity),
            };
        });

        monaco.editor.setModelMarkers(model, 'lemma-lsp', markers);
    }
}

function lspSeverityToMonaco(monaco, severity) {
    switch (severity) {
        case 1: return monaco.MarkerSeverity.Error;
        case 2: return monaco.MarkerSeverity.Warning;
        case 3: return monaco.MarkerSeverity.Info;
        case 4: return monaco.MarkerSeverity.Hint;
        default: return monaco.MarkerSeverity.Error;
    }
}
