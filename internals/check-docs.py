#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Documentation source/translation and assembled-artifact checks (no network)."""

import argparse
from collections import Counter
import json
from pathlib import Path
import sys
import tomllib
from xml.etree import ElementTree

from docs_html import Document, local_target
from docs_sitemap import check_sitemap, localized_sitemap
from docs_localization import (
    Locale,
    Page,
    Translation,
    check_config,
    check_translation,
    digest,
    parse_manifest,
    shared_config,
)

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "internals/docs-translations.json"


def sources(locale: Locale) -> dict[Page, str]:
    root = ROOT / "docs" / locale.value
    result = {}
    for path in sorted(root.rglob("*.md")):
        if not path.resolve().is_relative_to(root.resolve()):
            raise ValueError(f"Source escapes locale root: {path}")
        result[Page.parse(path.relative_to(root).as_posix())] = path.read_text(
            encoding="utf-8"
        )
    if not result:
        raise ValueError(f"No documentation found in {root}")
    return result


def load_manifest() -> dict[Page, Translation]:
    return parse_manifest(json.loads(MANIFEST.read_text(encoding="utf-8")))


def source_errors() -> list[str]:
    editions = {locale: sources(locale) for locale in Locale}
    english, japanese = editions[Locale.EN], editions[Locale.JA]
    manifest = load_manifest()
    errors = []
    for description, actual in [
        ("Japanese sources", japanese),
        ("translation manifest", manifest),
    ]:
        for page in sorted(set(english) - set(actual)):
            errors.append(f"{description}: missing {page.path}")
        for page in sorted(set(actual) - set(english)):
            errors.append(f"{description}: orphaned {page.path}")
    for page in sorted(set(english) & set(japanese)):
        errors.extend(check_translation(page, english[page], japanese[page]))
        if page in manifest:
            entry = manifest[page]
            if entry.english_sha256 != digest(english[page]):
                errors.append(
                    f"{page.path}: English changed; update the Japanese translation and record the pair"
                )
            if entry.japanese_sha256 != digest(japanese[page]):
                errors.append(
                    f"{page.path}: Japanese changed; record the updated pair for PR review"
                )
    configs = {}
    for locale, filename in [
        (Locale.EN, "zensical.toml"),
        (Locale.JA, "zensical.ja.toml"),
    ]:
        with (ROOT / filename).open("rb") as config_file:
            project = tomllib.load(config_file)["project"]
        configs[locale] = project
        errors.extend(check_config(project, locale, set(editions[locale])))
    if shared_config(configs[Locale.EN]) != shared_config(configs[Locale.JA]):
        errors.append("Non-localized Zensical settings or navigation structure differ")
    return errors


def record(paths: list[str]) -> None:
    english, japanese = sources(Locale.EN), sources(Locale.JA)
    manifest = load_manifest() if MANIFEST.exists() else {}
    for path in paths:
        page = Page.parse(path)
        if page not in english and page not in japanese:
            if page not in manifest:
                raise ValueError(f"Unknown page: {page.path}")
            del manifest[page]
            continue
        if page not in english or page not in japanese:
            raise ValueError(f"Both translations must exist: {page.path}")
        errors = check_translation(page, english[page], japanese[page])
        if errors:
            raise ValueError("\n".join(errors))
        manifest[page] = Translation(
            page, digest(english[page]), digest(japanese[page])
        )
    MANIFEST.write_text(
        json.dumps(
            {
                "version": 1,
                "pages": [entry.serialize() for _, entry in sorted(manifest.items())],
            },
            ensure_ascii=False,
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    print(
        f"Recorded {len(paths)} translation pairs. Human approval is the PR review/merge, not this command."
    )


def artifact_errors(site: Path) -> list[str]:
    site = site.resolve()
    pages = set(sources(Locale.EN))
    errors = []
    documents = {
        path.relative_to(site).as_posix(): Document(path.read_text(encoding="utf-8"))
        for path in sorted((site / "docs").rglob("*.html"))
    }
    expected_files = {
        locale.prefix.lstrip("/") + page.route + "index.html"
        for locale in Locale
        for page in pages
    }
    expected_files.update({"docs/404.html", "docs/ja/404.html"})
    if set(documents) != expected_files:
        errors.append(
            f"HTML inventory differs: missing={sorted(expected_files - set(documents))}, extra={sorted(set(documents) - expected_files)}"
        )
    for page in sorted(pages):
        pair = {}
        for locale in Locale:
            filename = page.url(locale).lstrip("/") + "index.html"
            if filename not in documents:
                continue
            document = documents[filename]
            pair[locale] = document
            if document.language != locale.value:
                errors.append(
                    f"{filename}: incorrect HTML language {document.language!r}"
                )
            canonicals = [
                link.target
                for link in document.links
                if link.tag == "link" and link.relation == "canonical"
            ]
            if canonicals != ["https://graphcal.org" + page.url(locale)]:
                errors.append(f"{filename}: incorrect canonical URL {canonicals}")
            alternates = [
                (link.language, link.target)
                for link in document.links
                if link.tag == "link" and link.relation == "alternate"
            ]
            if alternates:
                errors.append(
                    f"{filename}: use sitemap hreflang, not Zensical's site-root HTML alternates"
                )
            other = Locale.JA if locale == Locale.EN else Locale.EN
            if not any(
                link.tag == "a"
                and link.language == other.value
                and link.target == page.url(other)
                for link in document.links
            ):
                errors.append(f"{filename}: missing page-counterpart link")
        if len(pair) == len(Locale):
            en, ja = pair[Locale.EN], pair[Locale.JA]
            if en.headings != ja.headings:
                errors.append(
                    f"{page.path}: rendered heading levels/IDs differ; preserve stable English IDs in Japanese headings"
                )
            if en.table_rows != ja.table_rows:
                errors.append(
                    f"{page.path}: translated table rows missing/added ({en.table_rows} != {ja.table_rows})"
                )
            if Counter(en.inline_code) != Counter(ja.inline_code):
                errors.append(
                    f"{page.path}: inline code differs: missing={dict(Counter(en.inline_code) - Counter(ja.inline_code))}, added={dict(Counter(ja.inline_code) - Counter(en.inline_code))}"
                )
    # Check the real HTML rather than attempting to implement Markdown link syntax.
    # Production same-origin URLs are resolved against the local artifact, never fetched.
    for filename, document in documents.items():
        base = "/" + filename.removesuffix("index.html")
        for link in document.links:
            target = local_target(base, link.target)
            if target is None:
                continue
            path = site / target.path.lstrip("/")
            if path.is_dir():
                path /= "index.html"
            if not path.resolve().is_relative_to(site):
                errors.append(f"{filename}: link escapes artifact: {link.target}")
                continue
            if not path.is_file():
                errors.append(f"{filename}: missing local resource: {link.target}")
            elif (
                target.fragment
                and path.suffix == ".html"
                and not target.path.startswith("/playground/")
            ):
                key = path.relative_to(site).as_posix()
                if key not in documents:
                    documents_at_target = Document(path.read_text(encoding="utf-8"))
                else:
                    documents_at_target = documents[key]
                if target.fragment not in documents_at_target.identifiers:
                    errors.append(f"{filename}: missing fragment: {link.target}")
    for locale in Locale:
        path = site / locale.prefix.lstrip("/") / "sitemap.xml"
        errors.extend(check_sitemap(path.read_text(encoding="utf-8"), locale, pages))
        expected = {"https://graphcal.org" + page.url(locale) for page in pages}
        search = json.loads(
            (site / locale.prefix.lstrip("/") / "search.json").read_text(
                encoding="utf-8"
            )
        )
        if search["config"]["lang"] != [locale.value]:
            errors.append(f"{locale.value}: incorrect search language")
        search_pages = set()
        for item in search["items"]:
            target = local_target(locale.prefix, item["location"])
            if target is None:
                errors.append(
                    f"{locale.value}: external search result {item['location']}"
                )
                continue
            if item["level"] == 1:
                search_pages.add("https://graphcal.org" + target.path)
            filename = target.path.lstrip("/") + "index.html"
            document = documents.get(filename)
            if document is None or (
                target.fragment and target.fragment not in document.identifiers
            ):
                errors.append(
                    f"{locale.value}: missing search target {item['location']}"
                )
        if search_pages != expected:
            errors.append(
                f"{locale.value}: search index includes missing/foreign pages or omits pages"
            )
    sitemap_index = ElementTree.parse(site / "sitemap.xml").getroot()
    locations = [
        element.text
        for element in sitemap_index.iter(
            "{http://www.sitemaps.org/schemas/sitemap/0.9}loc"
        )
    ]
    if (
        sitemap_index.tag != "{http://www.sitemaps.org/schemas/sitemap/0.9}sitemapindex"
        or locations
        != ["https://graphcal.org" + locale.prefix + "sitemap.xml" for locale in Locale]
    ):
        errors.append("Root sitemap must discover both locale sitemaps")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser(
        "check",
        help="Check full translation coverage, freshness, examples, and configuration",
    )
    recorder = commands.add_parser(
        "record", help="Record explicitly named translated pairs for human PR review"
    )
    recorder.add_argument(
        "pages",
        nargs="+",
        help="Paths relative to a locale root, e.g. tutorial/index.md",
    )
    for name, description in [
        ("site", "Check built links, fragments, canonical URLs, and locale isolation"),
        ("sitemaps", "Add reciprocal page-equivalent hreflang to both built sitemaps"),
    ]:
        command = commands.add_parser(name, help=description)
        command.add_argument("--site", type=Path, default=ROOT / "site")
    args = parser.parse_args()
    try:
        if args.command == "record":
            record(args.pages)
            return 0
        if args.command == "sitemaps":
            pages = set(sources(Locale.EN))
            outputs = {}
            for locale in Locale:
                path = args.site / locale.prefix.lstrip("/") / "sitemap.xml"
                outputs[path] = localized_sitemap(
                    path.read_text(encoding="utf-8"), locale, pages
                )
            for path, content in outputs.items():
                path.write_text(content, encoding="utf-8")
            print("Annotated both locale sitemaps with reciprocal hreflang")
            return 0
        errors = (
            source_errors() if args.command == "check" else artifact_errors(args.site)
        )
        if errors:
            print("\n".join(sorted(set(errors))), file=sys.stderr)
            return 1
        print(
            f"Documentation {args.command} checks passed ({len(sources(Locale.EN))} pages per language)"
        )
        return 0
    except (ValueError, KeyError, TypeError, OSError, ElementTree.ParseError) as error:
        print(f"Documentation validation failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
