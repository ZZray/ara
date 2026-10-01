//! Actual per-request TLS effects for the fixed Bun fetch option carrier.
//! Cipher support is limited to explicitly named suites supported by ring;
//! the complete OpenSSL cipher expression language remains a parity gap.
use crate::{
    catalog_discovery::{
        DiscoveryBodyPolicy, DiscoveryError, DiscoveryHeaders, DiscoveryReply, DiscoveryRequest, HttpMethod,
    },
    model_collapse::VariantSpec,
};
use ara_rpc::{WireString, WireValue};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_rustls::{FixedServerNameResolver, HttpsConnector, HttpsConnectorBuilder};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use rustls::{
    ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime, pem::PemObject},
};
use std::{io::Read, sync::Arc};

fn error(message: impl ToString) -> DiscoveryError {
    DiscoveryError::named("TypeError", message.to_string())
}
fn text(value: &WireValue) -> Result<String, DiscoveryError> {
    value.as_string().map(|s| String::from_utf16_lossy(s.units())).ok_or_else(|| error("TLS value must be a string"))
}
fn pem_values(value: &WireValue) -> Result<Vec<Vec<u8>>, DiscoveryError> {
    match value {
        WireValue::Array(values) => values.iter().map(|value| text(value).map(String::into_bytes)).collect(),
        other => Ok(vec![text(other)?.into_bytes()]),
    }
}
fn certificates(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>, DiscoveryError> {
    let certificates =
        CertificateDer::pem_slice_iter(pem).map(|value| value.map_err(error)).collect::<Result<Vec<_>, _>>()?;
    if certificates.is_empty() {
        return Err(error("TLS certificate bundle is empty or invalid"));
    }
    Ok(certificates)
}
#[derive(Debug)]
struct UnverifiedChain {
    algorithms: WebPkiSupportedAlgorithms,
}
impl ServerCertVerifier for UnverifiedChain {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algorithms)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}
fn cipher_name(suite: rustls::CipherSuite) -> Option<&'static str> {
    use rustls::CipherSuite::*;
    match suite {
        TLS13_AES_128_GCM_SHA256 => Some("TLS_AES_128_GCM_SHA256"),
        TLS13_AES_256_GCM_SHA384 => Some("TLS_AES_256_GCM_SHA384"),
        TLS13_CHACHA20_POLY1305_SHA256 => Some("TLS_CHACHA20_POLY1305_SHA256"),
        TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 => Some("ECDHE-RSA-AES128-GCM-SHA256"),
        TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384 => Some("ECDHE-RSA-AES256-GCM-SHA384"),
        TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256 => Some("ECDHE-RSA-CHACHA20-POLY1305"),
        TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256 => Some("ECDHE-ECDSA-AES128-GCM-SHA256"),
        TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384 => Some("ECDHE-ECDSA-AES256-GCM-SHA384"),
        TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256 => Some("ECDHE-ECDSA-CHACHA20-POLY1305"),
        _ => None,
    }
}
fn tls_config(extra_ca: Option<&[u8]>, tls: &VariantSpec) -> Result<ClientConfig, DiscoveryError> {
    let mut provider = rustls::crypto::ring::default_provider();
    if let Some(value) = tls.get("ciphers").filter(|value| !matches!(value, WireValue::Null)) {
        let value = text(value)?;
        let requested: Vec<_> = value.split(':').collect();
        let mut selected = Vec::new();
        for name in requested {
            let Some(suite) =
                provider.cipher_suites.iter().find(|suite| cipher_name(suite.suite()) == Some(name)).copied()
            else {
                return Err(error(format!("Unsupported TLS cipher or OpenSSL cipher expression: {name}")));
            };
            if !selected.contains(&suite) {
                selected.push(suite);
            }
        }
        // Bun applies `ciphers` to the TLS 1.2 cipher list. TLS 1.3 uses its
        // separate default list even when the caller selects only TLS 1.2 names.
        let mut suites: Vec<_> =
            provider.cipher_suites.iter().filter(|suite| suite.version() == &rustls::version::TLS13).copied().collect();
        suites.extend(selected.into_iter().filter(|suite| suite.version() != &rustls::version::TLS13));
        provider.cipher_suites = suites;
    }
    let algorithms = provider.signature_verification_algorithms;
    let provider = Arc::new(provider);
    let mut roots = RootCertStore::empty();
    let curated = tls
        .get("ca")
        .filter(|value| !matches!(value, WireValue::Null))
        .filter(|value| !matches!(value,WireValue::Array(values)if values.is_empty()));
    if let Some(curated) = curated {
        for pem in pem_values(curated)? {
            for certificate in certificates(&pem)? {
                roots.add(certificate).map_err(error)?;
            }
        }
    } else {
        roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
        if let Some(pem) = extra_ca {
            for certificate in certificates(pem)? {
                roots.add(certificate).map_err(error)?;
            }
        }
    }
    let builder = ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions().map_err(error)?;
    let builder = if matches!(tls.get("rejectUnauthorized"), Some(WireValue::Bool(false))) {
        builder.dangerous().with_custom_certificate_verifier(Arc::new(UnverifiedChain { algorithms }))
    } else {
        builder.with_root_certificates(roots)
    };
    let cert = tls.get("cert").filter(|v| !matches!(v, WireValue::Null));
    let key = tls.get("key").filter(|v| !matches!(v, WireValue::Null));
    let chain = cert
        .map(|cert| {
            let mut chain = Vec::new();
            for pem in pem_values(cert)? {
                chain.extend(certificates(&pem)?);
            }
            Ok::<_, DiscoveryError>(chain)
        })
        .transpose()?;
    let key = key
        .map(|key| {
            let keys = pem_values(key)?;
            let joined: Vec<u8> =
                keys.iter().flat_map(|key| key.iter().copied().chain(std::iter::once(b'\n'))).collect();
            PrivateKeyDer::from_pem_slice(&joined).map_err(error)
        })
        .transpose()?;
    match (chain, key) {
        (Some(chain), Some(key)) => builder.with_client_auth_cert(chain, key).map_err(error),
        // Bun validates a supplied certificate/key independently and permits a
        // normal TLS connection when the server does not request client auth.
        _ => Ok(builder.with_no_client_auth()),
    }
}
type TlsClient = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;
fn client(config: ClientConfig, tls: Option<&VariantSpec>, http2: bool) -> Result<TlsClient, DiscoveryError> {
    let mut builder = HttpsConnectorBuilder::new().with_tls_config(config).https_or_http();
    if let Some(name) = tls.and_then(|tls| {
        tls.get("serverName")
            .filter(|v| !matches!(v, WireValue::Null))
            .or_else(|| tls.get("servername").filter(|v| !matches!(v, WireValue::Null)))
    }) {
        let name = ServerName::try_from(text(name)?).map_err(error)?;
        builder = builder.with_server_name_resolver(FixedServerNameResolver::new(name));
    }
    let connector = builder.enable_http1().enable_http2().build();
    let mut client_builder = Client::builder(TokioExecutor::new());
    if http2 {
        client_builder.http2_only(true);
    }
    Ok(client_builder.build(connector))
}
fn method(value: HttpMethod) -> http::Method {
    match value {
        HttpMethod::Get => http::Method::GET,
        HttpMethod::Post => http::Method::POST,
        HttpMethod::Put => http::Method::PUT,
        HttpMethod::Delete => http::Method::DELETE,
        HttpMethod::Head => http::Method::HEAD,
    }
}
fn decode_body(mut bytes: Vec<u8>, encoding: &str) -> Result<Vec<u8>, DiscoveryError> {
    for encoding in encoding.split(',').rev().map(str::trim) {
        let mut output = Vec::new();
        match encoding {
            "gzip" => {
                flate2::read::GzDecoder::new(bytes.as_slice()).read_to_end(&mut output).map_err(error)?;
            }
            "br" => {
                brotli::Decompressor::new(bytes.as_slice(), 4096).read_to_end(&mut output).map_err(error)?;
            }
            "deflate" => {
                if flate2::read::ZlibDecoder::new(bytes.as_slice()).read_to_end(&mut output).is_err() {
                    output.clear();
                    flate2::read::DeflateDecoder::new(bytes.as_slice()).read_to_end(&mut output).map_err(error)?;
                }
            }
            "zstd" => {
                output = zstd::stream::decode_all(bytes.as_slice()).map_err(error)?;
            }
            _ => continue,
        }
        bytes = output;
    }
    Ok(bytes)
}
pub async fn fetch(extra_ca: Option<&[u8]>, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
    let default_tls = VariantSpec::from_wire(WireValue::Object(Vec::new()));
    let tls = request.tls.as_ref().unwrap_or(&default_tls);
    let run = async {
        let mut http_client = None;
        let mut https_client = None;
        let mut url = reqwest::Url::parse(&String::from_utf16_lossy(request.url.units())).map_err(error)?;
        let mut headers = http::HeaderMap::new();
        for (key, value) in DiscoveryHeaders::from_pairs(&request.headers)?.entries() {
            let key =
                http::header::HeaderName::from_bytes(&key.units().iter().map(|unit| *unit as u8).collect::<Vec<_>>())
                    .map_err(error)?;
            let value = http::header::HeaderValue::from_bytes(
                &value.units().iter().map(|unit| *unit as u8).collect::<Vec<_>>(),
            )
            .map_err(error)?;
            headers.insert(key, value);
        }
        if !request.http2 && !headers.contains_key(http::header::ACCEPT_ENCODING) {
            headers.insert(http::header::ACCEPT_ENCODING, http::HeaderValue::from_static("gzip, deflate, br, zstd"));
        }
        let mut method = method(request.method);
        let mut body = request.body.clone().unwrap_or_default();
        let mut redirects = 0;
        loop {
            // Invalid TLS options are irrelevant for cleartext requests. Build
            // the TLS client only on an actual HTTPS hop, including redirects.
            let client = if url.scheme() == "https" {
                if https_client.is_none() {
                    https_client = Some(client(tls_config(extra_ca, tls)?, Some(tls), request.http2)?);
                }
                https_client.as_ref().expect("HTTPS client initialized")
            } else {
                if http_client.is_none() {
                    let config =
                        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                            .with_safe_default_protocol_versions()
                            .map_err(error)?
                            .with_root_certificates(RootCertStore::empty())
                            .with_no_client_auth();
                    http_client = Some(client(config, None, request.http2)?);
                }
                http_client.as_ref().expect("HTTP client initialized")
            };
            let mut outgoing = http::Request::builder().method(method.clone()).uri(url.as_str());
            *outgoing.headers_mut().expect("valid request builder") = headers.clone();
            let response = client
                .request(outgoing.body(Full::new(Bytes::from(body.clone()))).map_err(error)?)
                .await
                .map_err(error)?;
            let status = response.status().as_u16();
            if !request.http2
                && matches!(status, 301 | 302 | 303 | 307 | 308)
                && let Some(location) = response.headers().get(http::header::LOCATION)
            {
                let next = url.join(location.to_str().map_err(error)?).map_err(error)?;
                if redirects >= 126 {
                    return Err(error("Too many redirects"));
                }
                redirects += 1;
                if next.origin() != url.origin() {
                    for name in [http::header::AUTHORIZATION, http::header::COOKIE, http::header::PROXY_AUTHORIZATION] {
                        headers.remove(name);
                    }
                }
                headers.remove(http::header::HOST);
                if status == 303 && method != http::Method::HEAD
                    || matches!(status, 301 | 302) && method == http::Method::POST
                {
                    method = http::Method::GET;
                    body.clear();
                    for name in
                        [http::header::CONTENT_LENGTH, http::header::CONTENT_TYPE, http::header::TRANSFER_ENCODING]
                    {
                        headers.remove(name);
                    }
                }
                url = next;
                continue;
            }
            let consume = matches!(request.body_policy, DiscoveryBodyPolicy::Always) || (200..300).contains(&status);
            let encoding = if consume && !request.http2 {
                {
                    response
                        .headers()
                        .get(http::header::CONTENT_ENCODING)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_owned()
                }
            } else {
                Default::default()
            };
            let decoded = !encoding.is_empty()
                && encoding.split(',').all(|v| matches!(v.trim(), "gzip" | "deflate" | "br" | "zstd"));
            let mut reply_headers: Vec<(WireString, WireString)> = Vec::new();
            for (key, value) in response.headers() {
                if decoded && (key == http::header::CONTENT_ENCODING || key == http::header::CONTENT_LENGTH) {
                    continue;
                }
                let value = WireString::from_units(value.as_bytes().iter().map(|byte| u16::from(*byte)).collect());
                if let Some((_, previous)) = reply_headers.iter_mut().find(|(name, _)| name.equals_ascii(key.as_str()))
                {
                    previous.append_str(", ");
                    *previous = WireString::from_units([previous.units(), value.units()].concat());
                } else {
                    reply_headers.push((key.as_str().into(), value));
                }
            }
            let body = if consume {
                let bytes = response.into_body().collect().await.map_err(error)?.to_bytes().to_vec();
                decode_body(bytes, &encoding)?
            } else {
                Vec::new()
            };
            return Ok(DiscoveryReply { status, headers: reply_headers, body, json_override: None });
        }
    };
    if let Some(signal) = request.signal.clone() {
        tokio::select! {biased;reason=signal.cancelled()=>Err(reason),result=run=>result}
    } else {
        run.await
    }
}
