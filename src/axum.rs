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
        let fut = Box::new(self.router.call(r));

        TransformResponse::WaitResponse { fut }
    }
}

// To get autocompletion with rust-analyzer, set the option
// rust-analyzer.experimental.procAttrMacros to true
pin_project_lite::pin_project! {
    /// Future that will convert the hyper response body to the lambda_http's body
    ///
    /// This is used by the `AxumService` wrapper and is completely internal to the `service` function.
    #[project = TransformProj]
    #[doc(hidden)]
    pub enum TransformResponse {
        WaitResponse{ #[pin] fut: Box<RouterFuture<hyper::Body>> },
        WaitBody {
            parts: Option<http::response::Parts>,
            body: Pin<Box<dyn Future<Output = Result<axum::body::Bytes, axum::Error>>>>
        }
    }
}

impl Future for TransformResponse {
    type Output = Result<lambda_http::Response<lambda_http::Body>, lambda_http::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        match self.as_mut().project() {
            TransformProj::WaitResponse { fut } => {
                match fut.poll(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(result) => {
                        let response = result.unwrap(); // TODO

                        let (parts, body) = response.into_parts();
                        let parts = Some(parts);
                        let body = Box::pin(hyper::body::to_bytes(body));

                        // We got the response, switching to next polling phase: getting the body
                        self.set(TransformResponse::WaitBody { parts, body });

                        // TODO Do we need to wake up the waker ?
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                }
            }
            TransformProj::WaitBody { parts, body } => {
                match body.as_mut().poll(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(body) => {
                        let bytes = body.unwrap(); // TODO

                        let body = if bytes.is_empty() {
                            lambda_http::Body::Empty
                        } else {
                            lambda_http::Body::Binary(bytes.to_vec())
                        };

                        let parts = parts.take().expect("parts cannot be None");

                        let response = lambda_http::Response::from_parts(parts, body);

                        Poll::Ready(Ok(response))
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tower::Service;

    // Utility function to test out the TransformResponse implementation.
    // Because the struct takes a RouterFuture as a parameter, and because
    // that struct isn't intented to be created outside of axum, we resort
    // to creating a dummy Router which will create such future for us.
    async fn transform_response_test<F, Fut, Res>(axum_body: F, lambda_body: lambda_http::Body)
    where
        F: FnOnce() -> Fut + Clone + Send + 'static,
        Fut: std::future::Future<Output = Res> + Send,
        Res: axum::response::IntoResponse,
    {
        let mut app = axum::Router::new().route("/", axum::routing::get(axum_body));

        let request = http::Request::get("https://www.rust-lang.org/")
            .body(hyper::Body::empty())
            .unwrap();

        let fut = Box::new(app.call(request));

        let transform = super::TransformResponse::WaitResponse { fut };

        let res = transform.await;

        match res {
            Ok(res) => assert_eq!(res.into_body(), lambda_body),
            Err(e) => panic!("transform future resulted in an error: {:?}", e),
        }
    }

    #[tokio::test]
    async fn transform_empty_response() {
        transform_response_test(|| async { () }, lambda_http::Body::Empty).await;
    }

    #[tokio::test]
    async fn transform_one_chunk_response() {
        let bytes = "hello".as_bytes().to_owned();
        let b2 = bytes.clone();
        transform_response_test(move || async { b2 }, lambda_http::Body::Binary(bytes)).await;
    }

    #[tokio::test]
    async fn transform_multiple_chunks_response() {
        let chunks = vec!["Hello", " ", "World!"];
        let stream_chunks = chunks.clone();

        let bytes: Vec<u8> = chunks
            .into_iter()
            .flat_map(|s| s.to_string().into_bytes())
            .collect();

        transform_response_test(
            move || async {
                let stream_chunks: Vec<Result<_, std::io::Error>> = stream_chunks
                    .into_iter()
                    .map(|s| Ok(s.to_string()))
                    .collect();
                let stream = futures::stream::iter(stream_chunks);
                axum::body::StreamBody::new(stream)
            },
            lambda_http::Body::Binary(bytes),
        )
        .await;
    }
}
