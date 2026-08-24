"""Typed mutation-finding identities and baseline serialization."""

from __future__ import annotations

from dataclasses import dataclass, replace
from enum import StrEnum
import json
from pathlib import Path
import tomllib
from typing import Iterable


SCHEMA_VERSION = 1
FINDING_SUMMARIES = frozenset({"MissedMutant", "Timeout"})


class FindingStatus(StrEnum):
    OPEN = "open"
    RESOLVED = "resolved"
    OBSOLETE = "obsolete"
    EXCLUDED = "excluded"


class ReviewStatus(StrEnum):
    REVIEWED = "reviewed"
    UNREVIEWED = "unreviewed"


class Resolution(StrEnum):
    CAUGHT = "caught"
    UNVIABLE = "unviable"
    NOT_GENERATED = "not-generated"


@dataclass(frozen=True, order=True)
class SourcePosition:
    line: int
    column: int

    def __post_init__(self) -> None:
        if self.line < 0 or self.column < 1:
            raise ValueError(f"invalid function-relative source position: {self}")


@dataclass(frozen=True, order=True)
class SourceSpan:
    start: SourcePosition
    end: SourcePosition

    def __post_init__(self) -> None:
        if self.end < self.start:
            raise ValueError(f"source span ends before it starts: {self}")


@dataclass(frozen=True, order=True)
class MutationId:
    file: str
    function: str
    genre: str
    replacement: str
    span: SourceSpan


@dataclass(frozen=True)
class BaselineFinding:
    mutation_id: MutationId
    status: FindingStatus
    review: ReviewStatus
    rationale: str = ""
    resolution: Resolution | None = None

    def __post_init__(self) -> None:
        expected_resolution = self.status in {
            FindingStatus.RESOLVED,
            FindingStatus.OBSOLETE,
        }
        if expected_resolution != (self.resolution is not None):
            raise ValueError(
                f"status {self.status.value!r} and resolution {self.resolution!r} disagree"
            )
        if self.status is FindingStatus.OBSOLETE and self.resolution is not Resolution.NOT_GENERATED:
            raise ValueError("obsolete findings must use the not-generated resolution")
        if self.status is FindingStatus.RESOLVED and self.resolution is Resolution.NOT_GENERATED:
            raise ValueError("resolved findings require a tested resolution")
        if self.status is FindingStatus.EXCLUDED and not self.rationale:
            raise ValueError("excluded findings require a rationale")

    def reopen(self) -> BaselineFinding:
        return replace(
            self,
            status=FindingStatus.OPEN,
            review=ReviewStatus.UNREVIEWED,
            resolution=None,
        )

    def resolve(self, resolution: Resolution) -> BaselineFinding:
        if resolution is Resolution.NOT_GENERATED:
            raise ValueError("use mark_obsolete for findings that are no longer generated")
        return replace(
            self,
            status=FindingStatus.RESOLVED,
            resolution=resolution,
        )

    def mark_obsolete(self) -> BaselineFinding:
        return replace(
            self,
            status=FindingStatus.OBSOLETE,
            resolution=Resolution.NOT_GENERATED,
        )


@dataclass(frozen=True)
class MutationCandidate:
    mutation_id: MutationId
    name: str


@dataclass(frozen=True)
class MutantOutcome:
    mutation_id: MutationId
    summary: str


def _required_mapping(value: object, field: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ValueError(f"{field} must be an object")
    return value


def _required_string(mapping: dict[str, object], field: str) -> str:
    value = mapping.get(field)
    if not isinstance(value, str):
        raise ValueError(f"{field} must be a string")
    return value


def _required_int(mapping: dict[str, object], field: str) -> int:
    value = mapping.get(field)
    if not isinstance(value, int) or isinstance(value, bool):
        raise ValueError(f"{field} must be an integer")
    return value


def mutation_id_from_mutant(mutant_value: object) -> MutationId:
    mutant = _required_mapping(mutant_value, "mutant")
    function = _required_mapping(mutant.get("function"), "mutant.function")
    function_span = _required_mapping(function.get("span"), "mutant.function.span")
    function_start = _required_mapping(
        function_span.get("start"), "mutant.function.span.start"
    )
    mutant_span = _required_mapping(mutant.get("span"), "mutant.span")
    mutant_start = _required_mapping(mutant_span.get("start"), "mutant.span.start")
    mutant_end = _required_mapping(mutant_span.get("end"), "mutant.span.end")
    function_start_line = _required_int(function_start, "line")

    def relative_position(position: dict[str, object]) -> SourcePosition:
        return SourcePosition(
            line=_required_int(position, "line") - function_start_line,
            column=_required_int(position, "column"),
        )

    replacement = mutant.get("replacement", "")
    if not isinstance(replacement, str):
        raise ValueError("mutant.replacement must be a string")
    return MutationId(
        file=_required_string(mutant, "file"),
        function=_required_string(function, "function_name"),
        genre=_required_string(mutant, "genre"),
        replacement=replacement,
        span=SourceSpan(
            start=relative_position(mutant_start),
            end=relative_position(mutant_end),
        ),
    )


def candidate_from_json(value: object) -> MutationCandidate:
    mutant = _required_mapping(value, "candidate")
    return MutationCandidate(
        mutation_id=mutation_id_from_mutant(mutant),
        name=_required_string(mutant, "name"),
    )


def outcome_from_json(value: object) -> MutantOutcome | None:
    outcome = _required_mapping(value, "outcome")
    scenario_value = outcome.get("scenario")
    if not isinstance(scenario_value, dict):
        return None
    mutant = scenario_value.get("Mutant")
    if mutant is None:
        return None
    return MutantOutcome(
        mutation_id=mutation_id_from_mutant(mutant),
        summary=_required_string(outcome, "summary"),
    )


def load_candidates_json(contents: str) -> list[MutationCandidate]:
    value = json.loads(contents)
    if not isinstance(value, list):
        raise ValueError("cargo-mutants candidate listing must be an array")
    candidates = [candidate_from_json(candidate) for candidate in value]
    identities = [candidate.mutation_id for candidate in candidates]
    if len(identities) != len(set(identities)):
        raise ValueError("cargo-mutants produced duplicate structured mutation identities")
    return candidates


def load_outcomes(path: Path) -> list[MutantOutcome]:
    value = json.loads(path.read_text(encoding="utf-8"))
    report = _required_mapping(value, "report")
    outcomes = report.get("outcomes")
    if not isinstance(outcomes, list):
        raise ValueError("report.outcomes must be an array")
    return [
        outcome
        for raw_outcome in outcomes
        if (outcome := outcome_from_json(raw_outcome)) is not None
    ]


def load_finding_ids(paths: Iterable[Path]) -> set[MutationId]:
    return {
        outcome.mutation_id
        for path in paths
        for outcome in load_outcomes(path)
        if outcome.summary in FINDING_SUMMARIES
    }


def _position_from_toml(value: object, field: str) -> SourcePosition:
    mapping = _required_mapping(value, field)
    return SourcePosition(
        line=_required_int(mapping, "line"),
        column=_required_int(mapping, "column"),
    )


def _finding_from_toml(value: object) -> BaselineFinding:
    finding = _required_mapping(value, "finding")
    span = _required_mapping(finding.get("span"), "finding.span")
    resolution_value = finding.get("resolution")
    if resolution_value is not None and not isinstance(resolution_value, str):
        raise ValueError("finding.resolution must be a string")
    return BaselineFinding(
        mutation_id=MutationId(
            file=_required_string(finding, "file"),
            function=_required_string(finding, "function"),
            genre=_required_string(finding, "genre"),
            replacement=_required_string(finding, "replacement"),
            span=SourceSpan(
                start=_position_from_toml(span.get("start"), "finding.span.start"),
                end=_position_from_toml(span.get("end"), "finding.span.end"),
            ),
        ),
        status=FindingStatus(_required_string(finding, "status")),
        review=ReviewStatus(_required_string(finding, "review")),
        rationale=_required_string(finding, "rationale"),
        resolution=Resolution(resolution_value) if resolution_value is not None else None,
    )


def load_baseline(path: Path) -> dict[MutationId, BaselineFinding]:
    value = tomllib.loads(path.read_text(encoding="utf-8"))
    if value.get("schema_version") != SCHEMA_VERSION:
        raise ValueError(
            f"unsupported mutation baseline schema version: {value.get('schema_version')!r}"
        )
    raw_findings = value.get("finding")
    if not isinstance(raw_findings, list):
        raise ValueError("baseline.finding must be an array of tables")
    findings = [_finding_from_toml(raw_finding) for raw_finding in raw_findings]
    result = {finding.mutation_id: finding for finding in findings}
    if len(result) != len(findings):
        raise ValueError("mutation baseline contains duplicate identities")
    return result


def _toml_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def render_baseline(findings: Iterable[BaselineFinding]) -> str:
    lines = [
        f"schema_version = {SCHEMA_VERSION}",
        "",
        "# Mutation findings are retained as history. Automation changes status",
        "# instead of deleting records; only open and excluded findings are skipped",
        "# by ordinary discovery campaigns.",
    ]
    for finding in sorted(findings, key=lambda item: item.mutation_id):
        mutation_id = finding.mutation_id
        lines.extend(
            [
                "",
                "[[finding]]",
                f"file = {_toml_string(mutation_id.file)}",
                f"function = {_toml_string(mutation_id.function)}",
                f"genre = {_toml_string(mutation_id.genre)}",
                f"replacement = {_toml_string(mutation_id.replacement)}",
                "span = { "
                f"start = {{ line = {mutation_id.span.start.line}, column = {mutation_id.span.start.column} }}, "
                f"end = {{ line = {mutation_id.span.end.line}, column = {mutation_id.span.end.column} }} "
                "}",
                f"status = {_toml_string(finding.status.value)}",
                f"review = {_toml_string(finding.review.value)}",
                f"rationale = {_toml_string(finding.rationale)}",
            ]
        )
        if finding.resolution is not None:
            lines.append(f"resolution = {_toml_string(finding.resolution.value)}")
    return "\n".join(lines) + "\n"


def write_baseline(path: Path, findings: Iterable[BaselineFinding]) -> None:
    contents = render_baseline(findings)
    if not path.is_file() or path.read_text(encoding="utf-8") != contents:
        temporary = path.with_suffix(".tmp")
        temporary.write_text(contents, encoding="utf-8")
        temporary.replace(path)


def mutation_id_to_json(mutation_id: MutationId) -> dict[str, object]:
    return {
        "file": mutation_id.file,
        "function": mutation_id.function,
        "genre": mutation_id.genre,
        "replacement": mutation_id.replacement,
        "span": {
            "start": {
                "line": mutation_id.span.start.line,
                "column": mutation_id.span.start.column,
            },
            "end": {
                "line": mutation_id.span.end.line,
                "column": mutation_id.span.end.column,
            },
        },
    }


def mutation_id_from_json(value: object) -> MutationId:
    identity = _required_mapping(value, "mutation identity")
    span = _required_mapping(identity.get("span"), "mutation identity.span")
    return MutationId(
        file=_required_string(identity, "file"),
        function=_required_string(identity, "function"),
        genre=_required_string(identity, "genre"),
        replacement=_required_string(identity, "replacement"),
        span=SourceSpan(
            start=_position_from_toml(span.get("start"), "mutation identity.span.start"),
            end=_position_from_toml(span.get("end"), "mutation identity.span.end"),
        ),
    )


def display_mutation_id(mutation_id: MutationId) -> str:
    start = mutation_id.span.start
    end = mutation_id.span.end
    return (
        f"{mutation_id.file}: {mutation_id.function}: {mutation_id.genre} "
        f"{mutation_id.replacement!r} at +{start.line}:{start.column}-+{end.line}:{end.column}"
    )
