# Bounded native web research

2026-10-18, independent HandWave Node candidate 0.2.6. This product's pinned
rpi dependency and the root Core/OMP acceptance ledger are unchanged. Executed
results and publication status belong to [the delivery record](../evidence/web-research-0.2.6-20261018.md).

## Why separate research from shell and downloads

The previous Node could download files and run explicitly chosen interpreters,
but did not advertise a native web search or document reader. A similarly named
tool in the root Core did not make it available in this independent Host.
The Agent now receives `web_search` and `fetch_url` alongside its existing local
tools. They work in Execute, Plan and Goal under the existing Run budgets; Plan
still excludes downloads, scripts, installation and writes.

Default search uses DuckDuckGo Lite, an official non-JavaScript webpage, **not a
stable search API**. Availability, rate limits and regional access vary. A
challenge, format change or no usable results returns an explicit error, not a
fabricated answer or evidence of absence. No API/model credential is sent.
For a maintained search service, the Host may configure an existing SearXNG
endpoint using `HANDWAVE_SEARCH_URL=https://search.example.org/search`. Its JSON
format must be enabled; a public instance may refuse JSON. This variable is an
administrator configuration, not a tool argument. Only this search request may
use that trusted origin's private network; redirects cannot leave the origin.
This delivery does not install or deploy SearXNG.

`fetch_url` extracts UTF-8 HTML/text/JSON/XML without browser execution, login,
cookies or JavaScript. Binary/PDF content requires the separate download path.
It returns retrieval time, original/final URL, title, untrusted text, extracted
links, content SHA256 and UTF-8 byte pagination. `find` chooses a literal match
window. Pagination re-fetches the page: compare hashes before combining windows;
there is no snapshot cache. Extraction is lightweight, not a full HTML browser.

Read only relevant windows and a few primary sources, then cite actual returned
URLs. Search snippets are leads. The system prompt separates source facts from
inference, warns against sending private logs/secrets in queries, and treats
webpages/downloads as data incapable of granting authority or promoting memory.
This guidance does not constitute an OS sandbox or a proof against all prompt
injection. Existing shell/filesystem tools still have the launching account's
permissions.

## Network and output boundaries

Both tools use HTTP(S) GET, reject URL userinfo, limit each operation to 30 seconds
including DNS/headers/body, and respect cancellation. New research requests
validate every resolved address before connecting and pin direct connections to
those addresses, reject non-public ranges, follow at most three redirects, and
reject HTTPS downgrade. A configured HTTP/SOCKS proxy is trusted infrastructure:
its remote DNS/forwarding cannot be constrained by the Node's direct resolver.
Do not describe this mechanism as complete proxy-side SSRF isolation.

Input documents are capped at 1 MiB by actual streamed bytes, including responses
without Content-Length. Search JSON is bounded to 8 KiB. Fetch text defaults to
8,000 bytes, supports up to16,000 bytes, and has a48KiB serialized-output ceiling
including metadata/escaping. Script/style/navigation/footer text is removed.
Challenge/unsupported/oversize/error responses remain failures. Sensitive URL
query values are redacted in output; redacted links are not reusable credentials.

Existing `download_file` retains its128MiB/300-second, proxy and private-network
contract. Optional `expected_sha256` is validated before directory/network effects;
the streamed digest must match before replacing an existing destination. All
successful downloads return their actual SHA256. An observed local digest proves
the bytes read, **not publisher authenticity**: use a trusted official checksum.
Download success is separate from extraction, installation and version/self-test.

## Upstream references

- [DuckDuckGo non-JavaScript search](https://duckduckgo.com/duckduckgo-help-pages/features/non-javascript)
- [SearXNG search API and enabled formats](https://docs.searxng.org/dev/search_api.html)

The implementation does not copy a browser or third-party research framework,
add a Cargo dependency, or claim autonomous long-form research benchmarks.
