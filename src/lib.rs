/// Interop for the basic lambda event, without any HTTP layer shim.
/// I don't think there is much value here to be honest, because I'm
/// ready to bet that most of the actix ecosystem is around actix-http
/// and not raw services.
/// Still a good starting point :)
pub mod runtime {
    use lambda_runtime::{Error, LambdaEvent};
    use serde::{Deserialize, Serialize};
    use std::future::Future;

    pub async fn run_lambda_tower<A, B, F>(handler: F) -> Result<(), Error>
    where
        F: tower::Service<LambdaEvent<A>>,
        F::Future: Future<Output = Result<B, F::Error>>,
        F::Error: std::fmt::Debug + std::fmt::Display,
        A: for<'de> Deserialize<'de>,
        B: Serialize,
    {
        lambda_runtime::run(handler).await
    }

    pub async fn run_lambda_actix<A, B, F>(handler: F) -> Result<(), Error>
    where
        F: actix_web::dev::Service<LambdaEvent<A>, Response = B>,
        F::Error: std::fmt::Debug + std::fmt::Display,
        A: for<'de> Deserialize<'de>,
        B: Serialize,
    {
        let s = tower::service_fn(|req| handler.call(req));

        lambda_runtime::run(s).await
    }

    #[cfg(test)]
    mod tests {
        use super::{run_lambda_actix, run_lambda_tower};
        use lambda_runtime::{Error, LambdaEvent};
        use serde_json::{json, Value};

        async fn func(event: LambdaEvent<Value>) -> Result<Value, Error> {
            let (event, _context) = event.into_parts();
            let first_name = event["firstName"].as_str().unwrap_or("world");

            Ok(json!({ "message": format!("Hello, {}!", first_name) }))
        }

        #[allow(dead_code)]
        async fn main_tower() -> Result<(), Error> {
            let func = tower::service_fn(func);

            run_lambda_tower(func).await?;
            Ok(())
        }

        #[allow(dead_code)]
        async fn main_actix() -> Result<(), Error> {
            let func = actix_web::dev::fn_service(func);

            run_lambda_actix(func).await?;
            Ok(())
        }
    }
}
