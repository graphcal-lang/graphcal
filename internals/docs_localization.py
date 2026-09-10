"""Pure source/configuration checks for the two documentation editions."""

from dataclasses import dataclass
from enum import Enum
from hashlib import sha256
from pathlib import PurePosixPath
import re


class Locale(Enum):
    EN = "en"
    JA = "ja"

    @property
    def prefix(self) -> str:
        return {Locale.EN: "/docs/", Locale.JA: "/docs/ja/"}[self]


@dataclass(frozen=True, order=True)
class Page:
    """A canonical, repository-relative Markdown path within a locale root."""

    path: PurePosixPath

    def __post_init__(self):
        if (
            not isinstance(self.path, PurePosixPath)
            or self.path.is_absolute()
            or not re.fullmatch(r"[a-z0-9_/-]+\.md", str(self.path))
        ):
            raise ValueError(f"Invalid documentation page: {self.path!r}")

    @classmethod
    def parse(cls, value: str) -> "Page":
        if not isinstance(value, str) or not re.fullmatch(r"[a-z0-9_/-]+\.md", value):
            raise ValueError(f"Invalid documentation page: {value!r}")
        path = PurePosixPath(value)
        if path.is_absolute() or ".." in path.parts or str(path) != value:
            raise ValueError(f"Non-canonical documentation page: {value!r}")
        return cls(path)

    @property
    def route(self) -> str:
        if self.path.name == "index.md":
            return (
                "" if self.path.parent == PurePosixPath(".") else f"{self.path.parent}/"
            )
        return f"{self.path.with_suffix('')}/"

    def url(self, locale: Locale) -> str:
        return locale.prefix + self.route


def digest(content: str) -> str:
    return sha256(content.encode("utf-8")).hexdigest()


@dataclass(frozen=True)
class Translation:
    page: Page
    english_sha256: str
    japanese_sha256: str

    @classmethod
    def parse(cls, entry: object) -> "Translation":
        fields = {"path", "english_sha256", "japanese_sha256"}
        if not isinstance(entry, dict) or set(entry) != fields:
            raise ValueError(f"Invalid translation entry: {entry!r}")
        for key in fields - {"path"}:
            if not isinstance(entry[key], str) or not re.fullmatch(
                r"[0-9a-f]{64}", entry[key]
            ):
                raise ValueError(f"Invalid SHA-256 for {entry['path']}: {key}")
        return cls(
            Page.parse(entry["path"]), entry["english_sha256"], entry["japanese_sha256"]
        )

    def serialize(self) -> dict[str, str]:
        return {
            "path": str(self.page.path),
            "english_sha256": self.english_sha256,
            "japanese_sha256": self.japanese_sha256,
        }


def parse_manifest(value: object) -> dict[Page, Translation]:
    if (
        not isinstance(value, dict)
        or set(value) != {"version", "pages"}
        or type(value["version"]) is not int
        or value["version"] != 1
    ):
        raise ValueError("Expected translation manifest version 1")
    if not isinstance(value["pages"], list):
        raise ValueError("Manifest pages must be an array")
    entries = [Translation.parse(entry) for entry in value["pages"]]
    result = {entry.page: entry for entry in entries}
    if len(result) != len(entries):
        raise ValueError("Duplicate translation paths")
    return result


@dataclass(frozen=True)
class Heading:
    line: int
    level: int
    text: str


@dataclass(frozen=True)
class MarkdownSurface:
    headings: tuple[Heading, ...]
    examples: tuple[str, ...]


def markdown_surface(source: str) -> MarkdownSurface:
    """Read ATX headings and fenced examples, not general Markdown/link syntax.

    Links/fragments and rendered structure are checked on Zensical's actual HTML.
    Keeping fence bytes (including info strings/comments) deliberately makes
    executable examples identical across editions.
    """
    headings: list[Heading] = []
    examples: list[str] = []
    fence: str | None = None
    block: list[str] = []
    for number, line in enumerate(source.splitlines(keepends=True)):
        if fence is not None:
            block.append(line)
            if re.fullmatch(
                r"[ \t]*"
                + re.escape(fence[0])
                + "{"
                + str(len(fence))
                + r",}[ \t]*\n?",
                line,
            ):
                examples.append("".join(block))
                fence, block = None, []
        else:
            opening = re.match(r"^[ \t]*(`{3,}|~{3,})", line)
            if opening:
                fence, block = opening[1], [line]
            else:
                heading = re.match(r"^(#{1,6})[ \t]+(.+?)\s*$", line)
                if heading:
                    headings.append(Heading(number, len(heading[1]), heading[2]))
    if fence is not None:
        raise ValueError("Unclosed Markdown code fence")
    return MarkdownSurface(tuple(headings), tuple(examples))


def check_translation(page: Page, english: str, japanese: str) -> list[str]:
    en, ja = markdown_surface(english), markdown_surface(japanese)
    errors = []
    if en.examples != ja.examples:
        errors.append(f"{page.path}: fenced examples differ between languages")
    if [h.level for h in en.headings] != [h.level for h in ja.headings]:
        errors.append(f"{page.path}: heading structure differs between languages")
    if not re.search(r"[\u3040-\u30ff\u3400-\u9fff]", japanese):
        errors.append(f"{page.path}: no Japanese text")
    return errors


def nav_structure(value: object) -> object:
    """Discard localized labels while preserving ordering and section nesting."""
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return [nav_structure(item) for item in value]
    if isinstance(value, dict) and len(value) == 1:
        label, target = next(iter(value.items()))
        if not isinstance(label, str) or not label.strip():
            raise ValueError("Navigation labels must be nonempty strings")
        return nav_structure(target)
    raise ValueError(f"Invalid navigation: {value!r}")


def nav_targets(value: object) -> list[str]:
    structure = nav_structure(value)
    if isinstance(structure, str):
        return [structure]
    return [target for item in structure for target in nav_targets(item)]


def shared_config(project: dict) -> dict:
    """All configuration except explicitly locale-dependent values must agree."""
    result = {
        key: value
        for key, value in project.items()
        if key
        not in {
            "site_name",
            "site_description",
            "site_url",
            "docs_dir",
            "site_dir",
            "theme",
            "nav",
        }
    }
    result["nav"] = nav_structure(project["nav"])
    theme = {
        key: value
        for key, value in project["theme"].items()
        if key not in {"language", "palette"}
    }
    theme["palette"] = [
        {
            **palette,
            "toggle": {
                key: value for key, value in palette["toggle"].items() if key != "name"
            },
        }
        for palette in project["theme"]["palette"]
    ]
    result["theme"] = theme
    return result


def check_config(project: dict, locale: Locale, pages: set[Page]) -> list[str]:
    expected = {
        "site_url": "https://graphcal.org" + locale.prefix,
        "docs_dir": f"docs/{locale.value}",
        "site_dir": f"target/docs-site/{locale.value}",
    }
    errors = [
        f"{locale.value}: {key} must be {value!r}"
        for key, value in expected.items()
        if project.get(key) != value
    ]
    if project["theme"]["language"] != locale.value:
        errors.append(f"{locale.value}: incorrect theme language")
    for target in nav_targets(project["nav"]):
        if target == "https://graphcal.org/playground/":
            continue
        if Page.parse(target) not in pages:
            errors.append(f"{locale.value}: missing navigation target {target}")
    expected_alternates = [
        {"name": "English", "link": Locale.EN.prefix, "lang": Locale.EN.value},
        {"name": "日本語", "link": Locale.JA.prefix, "lang": Locale.JA.value},
    ]
    if project["extra"].get("alternate") != expected_alternates:
        errors.append(f"{locale.value}: incorrect alternate-language configuration")
    return errors
