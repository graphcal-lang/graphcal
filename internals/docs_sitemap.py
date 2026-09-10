"""Page-equivalent language metadata at the sitemap serialization boundary."""

from xml.etree import ElementTree
from xml.sax.saxutils import escape

from docs_localization import Locale, Page

SITEMAP = "http://www.sitemaps.org/schemas/sitemap/0.9"
XHTML = "http://www.w3.org/1999/xhtml"
ORIGIN = "https://graphcal.org"


def sitemap_locations(xml: str) -> list[str]:
    root = ElementTree.fromstring(xml)
    if root.tag != f"{{{SITEMAP}}}urlset":
        raise ValueError("Expected a sitemap urlset")
    return [element.text for element in root.iter(f"{{{SITEMAP}}}loc")]


def localized_sitemap(original: str, locale: Locale, pages: set[Page]) -> str:
    """Validate the builder's inventory before adding standard xhtml alternates.

    HTML head alternates in Zensical 0.0.58 also drive its client-side locale
    router, which treats every alternate as a site root and fetches sitemap.xml
    below it. Use the equivalent standard sitemap hreflang mechanism instead.
    """
    actual = sitemap_locations(original)
    expected = {ORIGIN + page.url(locale) for page in pages}
    if set(actual) != expected or len(actual) != len(expected):
        raise ValueError(
            f"{locale.value}: cannot annotate an incomplete or stale sitemap"
        )
    rows = []
    for page in sorted(pages):
        links = "\n".join(
            f'    <xhtml:link rel="alternate" hreflang="{other.value}" href="{escape(ORIGIN + page.url(other))}" />'
            for other in Locale
        )
        rows.append(
            f"  <url>\n    <loc>{escape(ORIGIN + page.url(locale))}</loc>\n{links}\n  </url>"
        )
    return (
        f'<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="{SITEMAP}" xmlns:xhtml="{XHTML}">\n'
        + "\n".join(rows)
        + "\n</urlset>\n"
    )


def check_sitemap(xml: str, locale: Locale, pages: set[Page]) -> list[str]:
    locations = sitemap_locations(xml)
    expected = {ORIGIN + page.url(locale): page for page in pages}
    if set(locations) != set(expected) or len(locations) != len(expected):
        return [f"{locale.value}: sitemap does not match page inventory"]
    errors = []
    for row in ElementTree.fromstring(xml).findall(f"{{{SITEMAP}}}url"):
        location = row.findtext(f"{{{SITEMAP}}}loc")
        page = expected[location]
        actual = [link.attrib for link in row.findall(f"{{{XHTML}}}link")]
        alternates = [
            {
                "rel": "alternate",
                "hreflang": other.value,
                "href": ORIGIN + page.url(other),
            }
            for other in Locale
        ]
        if actual != alternates:
            errors.append(
                f"{locale.value}: missing/incorrect sitemap hreflang for {page.path}"
            )
    return errors
