"""Parse rendered documentation at the HTML boundary; no filesystem access."""

from dataclasses import dataclass
from html.parser import HTMLParser
from urllib.parse import unquote, urljoin, urlsplit


@dataclass(frozen=True)
class Link:
    target: str
    tag: str
    relation: str = ""
    language: str = ""


@dataclass(frozen=True)
class LocalTarget:
    path: str
    fragment: str


def local_target(base: str, target: str) -> LocalTarget | None:
    """External resources have no local target; malformed local URLs are errors."""
    url = urlsplit(urljoin("https://graphcal.org" + base, target))
    if url.scheme not in {"http", "https"} or url.netloc != "graphcal.org":
        return None
    path = unquote(url.path)
    if (
        not path.startswith("/")
        or "\\" in path
        or "\x00" in path
        or any(part in {".", ".."} for part in path.split("/"))
    ):
        raise ValueError(f"Unsafe local URL: {target!r}")
    return LocalTarget(path, unquote(url.fragment))


@dataclass(frozen=True)
class HtmlHeading:
    level: int
    identifier: str


class Document(HTMLParser):
    def __init__(self, html: str):
        super().__init__(convert_charrefs=True)
        self.language: str | None = None
        self.identifiers: set[str] = set()
        self.links: list[Link] = []
        self.headings: list[HtmlHeading] = []
        self.table_rows = 0
        self.inline_code: list[str] = []
        self._article = False
        self._pre = False
        self._code: list[str] | None = None
        self.feed(html)
        self.close()

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "html":
            self.language = attrs.get("lang")
        if identifier := attrs.get("id"):
            self.identifiers.add(identifier)
        for attribute in ("href", "src"):
            if target := attrs.get(attribute):
                self.links.append(
                    Link(target, tag, attrs.get("rel", ""), attrs.get("hreflang", ""))
                )
        if tag == "article":
            self._article = True
        if self._article:
            if tag in {"h1", "h2", "h3", "h4", "h5", "h6"}:
                self.headings.append(HtmlHeading(int(tag[1]), attrs.get("id", "")))
            if tag == "tr":
                self.table_rows += 1
            if tag == "pre":
                self._pre = True
            if tag == "code" and not self._pre:
                self._code = []

    def handle_endtag(self, tag):
        if tag == "article":
            self._article = False
        if tag == "pre":
            self._pre = False
        if tag == "code" and self._code is not None:
            # Inline HTML whitespace collapses; prose line wrapping is not a code change.
            self.inline_code.append(" ".join("".join(self._code).split()))
            self._code = None

    def handle_data(self, data):
        if self._code is not None:
            self._code.append(data)
