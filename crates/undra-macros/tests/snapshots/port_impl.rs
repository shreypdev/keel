impl Http for FakeHttp {
    fn request(
        &self,
        req: HttpRequest,
    ) -> ::core::pin::Pin<
        ::std::boxed::Box<
            dyn ::core::future::Future<
                Output = Result<HttpResponse, HttpError>,
            > + ::core::marker::Send + '_,
        >,
    > {
        ::std::boxed::Box::pin(async move { todo!() })
    }
    fn other(&self) {}
}
