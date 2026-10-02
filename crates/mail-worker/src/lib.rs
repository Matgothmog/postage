//! Cloudflare Worker entry point (workers-rs).

use worker::{Context, Env, Request, Response, Result, event};

#[event(fetch)]
async fn fetch(_req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    Response::ok("postage-mail")
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds_and_tests_run() {
        assert_eq!(2 + 2, 4);
    }
}
