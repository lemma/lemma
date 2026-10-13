mod documentation_examples;
mod documentation_fences;
mod documentation_formatting;
mod evaluate;
mod examples;
mod lists;
mod llms_txt;
mod lsp;
mod lsp_session;
mod mcp;
mod mcp_http;
mod ordering;
mod server;

fn stop_child(child: &mut std::process::Child) {
    if child
        .try_wait()
        .expect("BUG: query test child status")
        .is_none()
    {
        child.kill().expect("BUG: kill test child");
        child.wait().expect("BUG: reap test child");
    }
}
