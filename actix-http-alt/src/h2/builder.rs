use std::future::Future;

use actix_service_alt::ServiceFactory;
use bytes::Bytes;
use futures_core::Stream;
use http::{Request, Response};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::body::ResponseBody;
use crate::builder::HttpServiceBuilder;
use crate::error::{BodyError, HttpServiceError};
use crate::response::ResponseError;

use super::body::RequestBody;
use super::service::H2Service;

/// Http/1 Builder type.
/// Take in generic types of ServiceFactory for http and tls.
pub type H2ServiceBuilder<F, FE, FU, FA, const READ_BUF_LIMIT: usize, const WRITE_BUF_LIMIT: usize> =
    HttpServiceBuilder<F, RequestBody, FE, FU, FA, READ_BUF_LIMIT, WRITE_BUF_LIMIT>;

impl<F, FE, FU, FA, const READ_BUF_LIMIT: usize, const WRITE_BUF_LIMIT: usize>
    HttpServiceBuilder<F, RequestBody, FE, FU, FA, READ_BUF_LIMIT, WRITE_BUF_LIMIT>
{
    #[cfg(feature = "openssl")]
    pub fn openssl(
        self,
        acceptor: crate::tls::openssl::TlsAcceptor,
    ) -> H2ServiceBuilder<F, FE, FU, crate::tls::openssl::TlsAcceptorService, READ_BUF_LIMIT, WRITE_BUF_LIMIT> {
        H2ServiceBuilder {
            factory: self.factory,
            expect: self.expect,
            upgrade: self.upgrade,
            tls_factory: crate::tls::openssl::TlsAcceptorService::new(acceptor),
            config: self.config,
            _body: std::marker::PhantomData,
        }
    }

    #[cfg(feature = "rustls")]
    pub fn rustls(
        self,
        config: crate::tls::rustls::RustlsConfig,
    ) -> H2ServiceBuilder<F, FE, FU, crate::tls::rustls::TlsAcceptorService, READ_BUF_LIMIT, WRITE_BUF_LIMIT> {
        H2ServiceBuilder {
            factory: self.factory,
            expect: self.expect,
            upgrade: self.upgrade,
            tls_factory: crate::tls::rustls::TlsAcceptorService::new(config),
            config: self.config,
            _body: std::marker::PhantomData,
        }
    }

    #[cfg(feature = "native-tls")]
    pub fn native_tls(
        self,
        acceptor: crate::tls::native_tls::TlsAcceptor,
    ) -> H2ServiceBuilder<F, FE, FU, crate::tls::native_tls::TlsAcceptorService, READ_BUF_LIMIT, WRITE_BUF_LIMIT> {
        H2ServiceBuilder {
            factory: self.factory,
            expect: self.expect,
            upgrade: self.upgrade,
            tls_factory: crate::tls::native_tls::TlsAcceptorService::new(acceptor),
            config: self.config,
            _body: std::marker::PhantomData,
        }
    }
}

impl<St, F, B, E, FE, FU, FA, TlsSt, const READ_BUF_LIMIT: usize, const WRITE_BUF_LIMIT: usize> ServiceFactory<St>
    for H2ServiceBuilder<F, FE, FU, FA, READ_BUF_LIMIT, WRITE_BUF_LIMIT>
where
    F: ServiceFactory<Request<RequestBody>, Response = Response<ResponseBody<B>>>,
    F::Service: 'static,

    F::Error: ResponseError<F::Response>,

    F::InitError: From<FA::InitError>,

    FA: ServiceFactory<St, Response = TlsSt, Config = ()>,
    FA::Service: 'static,
    HttpServiceError: From<FA::Error>,

    B: Stream<Item = Result<Bytes, E>> + 'static,
    E: 'static,
    BodyError: From<E>,

    St: AsyncRead + AsyncWrite + Unpin,
    TlsSt: AsyncRead + AsyncWrite + Unpin,
{
    type Response = ();
    type Error = HttpServiceError;
    type Config = F::Config;
    type Service = H2Service<F::Service, FA::Service, READ_BUF_LIMIT, WRITE_BUF_LIMIT>;
    type InitError = F::InitError;
    type Future = impl Future<Output = Result<Self::Service, Self::InitError>>;

    fn new_service(&self, cfg: Self::Config) -> Self::Future {
        let service = self.factory.new_service(cfg);
        let tls_acceptor = self.tls_factory.new_service(());
        let config = self.config;

        async move {
            let service = service.await?;
            let tls_acceptor = tls_acceptor.await?;

            Ok(H2Service::new(config, service, (), None, tls_acceptor))
        }
    }
}
