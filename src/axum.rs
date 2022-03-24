use axum::{routing::future::RouterFuture, Router};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

pub struct AxumService {
    router: Router,
}

pub async fn service(router: Router) -> AxumService {
    AxumService { router }
}

impl tower::Service<lambda_http::Request> for AxumService {
    type Response = lambda_http::Response<lambda_http::Body>;
    type Error = lambda_http::Error;

    type Future = TransformResponse;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.router
            .poll_ready(cx)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    fn call(&mut self, req: lambda_http::Request) -> Self::Future {
        let r = req.map(|b| match b {
            lambda_http::Body::Empty => hyper::Body::empty(),
            lambda_http::Body::Text(s) => s.into(),
            lambda_http::Body::Binary(b) => b.into(),
        });
        let fut = Box::pin(self.router.call(r));

        TransformResponse { fut }
    }
}

/// Future that will convert the hyper response body to the lambda_http's body
///
/// This is used by the `AxumService` wrapper and is completely internal to the `service` function.
#[doc(hidden)]
pub struct TransformResponse {
    fut: Pin<Box<RouterFuture<hyper::Body>>>,
}

impl Future for TransformResponse {
    type Output = Result<lambda_http::Response<lambda_http::Body>, lambda_http::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        match self.fut.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                let r = result.unwrap();
                let r = r.map(|_body| {
                    todo!("How do I accumulate the body here ?") // TODO
                });

                Poll::Ready(Ok(r))
            }
        }
    }
}
