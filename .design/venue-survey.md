# Academic Venue Survey for Graphcal

*Last updated: 2026-02-17*

This document surveys academic journals and conferences suitable for presenting Graphcal's contributions. Venues are organized by research community, with notes on which aspects of Graphcal each venue is best suited for.

## Summary of Graphcal's Publishable Contributions

| Contribution | Key Novelty |
|---|---|
| Multi-layer type system (6 orthogonal layers) | Composable type layers: primitives, dimensions, units, ADTs, spaces (phantom types), indexes |
| Compile-time dimensional analysis | Dimension algebra as first-class type system feature, extending Numbat to a full language |
| Space safety via phantom types | General-purpose phantom type parameters instead of special-purpose tag systems (cf. Sguaba) |
| Reactive DAG with incremental recomputation | Hybrid eager/lazy eval with dirty tracking and early cutoff (backdating), inspired by Salsa/Typst |
| System dynamics as pattern, not keywords | Temporal simulation via `index`/`scan`/struct — no stock/flow keywords (contrast with Vensim) |
| Git-friendly language design | First language designed ground-up for diffable, versionable engineering calculations |
| Scenario management as language feature | `.scenario` overlay files replacing spreadsheet versioning anti-patterns |
| N-dimensional labeled tables | Tables as language primitives with map/reduce/scan and dimensional analysis |

---

## Tier 1: Programming Languages (Core PL Community)

These are the top venues for the language design and type system contributions.

### Conferences

#### PLDI — ACM SIGPLAN Conference on Programming Language Design and Implementation
- **Scope:** Premier forum for PL design and implementation — compilers, type systems, program analysis, runtime systems.
- **Fit for Graphcal:** The incremental computation model (hybrid eager/lazy with early cutoff), the 6-layer type system design, compile-time dimension algebra.
- **Frequency:** Annual (June). PLDI 2026: Boulder, Colorado, June 15–19.
- **Submission:** Publishes in PACMPL. Competitive (acceptance ~20%).
- **Link:** https://conf.researchr.org/series/pldi

#### POPL — ACM SIGPLAN Symposium on Principles of Programming Languages
- **Scope:** Fundamental innovations in PL design, definition, analysis, transformation, and implementation. Strongest in formal/theoretical work.
- **Fit for Graphcal:** Formal treatment of the dimension type algebra, space safety via phantom type parameters, purity enforcement. Requires strong theoretical contribution.
- **Frequency:** Annual (January). POPL 2026 was in Rennes, France. POPL 2027 TBA.
- **Submission:** Publishes in PACMPL. Very competitive (~20% acceptance).
- **Link:** https://www.sigplan.org/Conferences/

#### OOPSLA — ACM SIGPLAN Conference on Object-Oriented Programming, Systems, Languages, and Applications
- **Scope:** Practical and theoretical investigations of programming systems, languages, and environments. Broader than POPL — welcomes implementations and evaluations.
- **Fit for Graphcal:** Full language design paper (type system + reactive model + scenario management). OOPSLA is more receptive to "whole system" papers than PLDI/POPL.
- **Frequency:** Annual at SPLASH (October). SPLASH 2026: October 4–9 (location TBA).
- **Submission:** Two rounds — Round 1 deadline Oct 10, 2025; Round 2 deadline Mar 17, 2026. Publishes in PACMPL.
- **Link:** https://conf.researchr.org/track/splash-2026/oopsla-2026

#### ICFP — ACM SIGPLAN International Conference on Functional Programming
- **Scope:** Design, implementation, and uses of functional programming.
- **Fit for Graphcal:** Pure function model, dimension generics, reactive/dataflow evaluation (related to FRP). Good fit if Graphcal's functional aspects are foregrounded.
- **Frequency:** Annual (September/October). ICFP 2025 was in Singapore.
- **Submission:** Publishes in PACMPL.
- **Link:** https://icfp25.sigplan.org/

#### ECOOP — European Conference on Object-Oriented Programming
- **Scope:** All practical and theoretical investigations of programming languages, systems, and environments. Europe's longest-standing annual PL conference.
- **Fit for Graphcal:** Similar to OOPSLA. Good for the whole-language design story.
- **Frequency:** Annual (June/July). ECOOP 2026: Brussels, Belgium, June 29 – July 3.
- **Submission:** Published in LIPIcs (Dagstuhl). Open access.
- **Link:** https://2026.ecoop.org/

#### ESOP — European Symposium on Programming
- **Scope:** Fundamental issues in specification, design, analysis, and implementation of PLs. Part of ETAPS.
- **Fit for Graphcal:** Formal aspects of the type system (dimension algebra, phantom types for spaces). More theory-oriented than ECOOP.
- **Frequency:** Annual (April, at ETAPS).
- **Submission:** Published in Springer LNCS. Two-round scheme.
- **Link:** https://etaps.org/2026/conferences/esop/

### Journals

#### ACM TOPLAS — Transactions on Programming Languages and Systems
- **Scope:** Premier journal for PL research. Covers language design, implementation, semantics, compilers, type systems, testing, verification.
- **Fit for Graphcal:** Comprehensive paper on the full type system design (all 6 layers), or focused paper on dimension type algebra and space safety. Also has a "Tools, Systems and Practitioner Reports" track.
- **Review time:** Typically 3–6 months.
- **Link:** https://dl.acm.org/journal/toplas

#### JFP — Journal of Functional Programming
- **Scope:** Design, implementation, and application of functional programming languages. Published by Cambridge University Press (moving to Diamond OA via Episciences from 2026).
- **Fit for Graphcal:** Pure function model, dataflow evaluation, dimension generics. Good for a paper focused on Graphcal's functional foundations.
- **Link:** https://www.cambridge.org/core/journals/journal-of-functional-programming

#### PACMPL — Proceedings of the ACM on Programming Languages
- **Scope:** The journal vehicle for POPL, PLDI, OOPSLA, and ICFP. Papers are submitted to the conference but published in PACMPL.
- **Note:** Submit through one of the four conferences above; papers appear in PACMPL.
- **Link:** https://dl.acm.org/journal/pacmpl

---

## Tier 2: Domain-Specific Languages & Software Language Engineering

These venues focus specifically on the design, implementation, and engineering of new languages — especially DSLs.

### Conferences

#### SLE — ACM SIGPLAN International Conference on Software Language Engineering
- **Scope:** Principles of software languages: design, implementation, evolution. Core venue for DSL research.
- **Fit for Graphcal:** Strongest fit for a paper on Graphcal as a DSL for engineering calculations — language design, implementation techniques, the Git-friendly design decisions.
- **Frequency:** Annual (October, co-located with SPLASH). SLE 2026 TBA.
- **Link:** https://conf.researchr.org/series/sle

#### GPCE — ACM SIGPLAN International Conference on Generative Programming: Concepts & Experiences
- **Scope:** Code generation, language implementation, model-driven engineering, product-line development. Accepts full papers, tool demos, and "Generative Pearls."
- **Fit for Graphcal:** Scenario management as a product-line concept; code generation from `.graph` files; the "Generative Pearl" format is excellent for an elegant exposition of Graphcal's design.
- **Frequency:** Annual. GPCE 2026: co-located with ECOOP 2026 in Brussels.
- **Link:** https://2026.ecoop.org/home/gpce-2026

#### Onward! — (at SPLASH)
- **Scope:** Grand visions and new paradigms for programming. More radical and visionary than OOPSLA — accepts less rigorous validation (compelling arguments, exploratory implementations, worked examples).
- **Fit for Graphcal:** Ideal for a vision paper on "what engineering calculation languages should look like." The Onward! Essays track is perfect for a narrative on the journey from spreadsheets to Graphcal.
- **Frequency:** Annual at SPLASH (October).
- **Link:** https://www.sigplan.org/Conferences/Onward/

#### CC — ACM SIGPLAN International Conference on Compiler Construction
- **Scope:** Processing programs in the most general sense: analyzing, transforming, executing input that describes how a system operates.
- **Fit for Graphcal:** Static dependency extraction for the DAG, compile-time dimension checking, incremental recompilation.
- **Frequency:** Annual (February/March).
- **Link:** https://conf.researchr.org/series/CC

### Journals

#### The Art, Science, and Engineering of Programming
- **Scope:** Anything about programming — libraries, frameworks, languages, APIs, programming models, pearls, and essays. Open access, overlay on arXiv. Accepts "Art" (practical/experiential), "Science" (formal), and "Engineering" (measured/evaluated) perspectives.
- **Fit for Graphcal:** Excellent venue for a full "language design experience" paper. The "Art" perspective welcomes programming pearls and language design essays. Lower barrier than TOPLAS but still peer-reviewed.
- **Submission deadline:** Feb 1, 2026 (rolling cycles).
- **Link:** https://programming-journal.org

#### SoSyM — Software and Systems Modeling (Springer)
- **Scope:** Theory and practice of modeling languages and techniques for software and engineered systems. Covers model-driven engineering, DSLs, metamodeling, formal syntax/semantics.
- **Fit for Graphcal:** The reactive DAG as a computation model, Graphcal's syntax and semantics design, comparison with spreadsheet and Vensim modeling paradigms.
- **Link:** https://link.springer.com/journal/10270

---

## Tier 3: Software Engineering

For the Git-friendly design, scenario management, and engineering tooling aspects.

### Conferences

#### ICSE — IEEE/ACM International Conference on Software Engineering
- **Scope:** Premier SE conference. Research results, innovations, trends, experiences in software engineering.
- **Fit for Graphcal:** "Engineering calculations as code" — Git workflows, CI/CD integration, scenario testing, reproducibility. Best as a "Software Engineering in Practice" (SEIP) track paper.
- **Frequency:** Annual (April/May). ICSE 2026: Rio de Janeiro, April 12–18.
- **Link:** https://conf.researchr.org/home/icse-2026

#### ESEC/FSE — ACM Joint European Software Engineering Conference / Symposium on the Foundations of Software Engineering
- **Scope:** Foundations and practices of SE. Strong in tooling, program analysis, empirical SE.
- **Fit for Graphcal:** Similar to ICSE. The "ideas, visions, and reflections" track could work for the vision of replacing spreadsheets with a proper language.
- **Frequency:** Annual.
- **Link:** https://conf.researchr.org/series/fse

#### ASE — IEEE/ACM International Conference on Automated Software Engineering
- **Scope:** Automated SE techniques and tools.
- **Fit for Graphcal:** Incremental recomputation, static dependency analysis, automated scenario comparison/regression testing.
- **Frequency:** Annual.

### Journals

#### IEEE TSE — Transactions on Software Engineering
- **Scope:** Leading SE journal. Well-defined theoretical results and empirical studies impacting construction, analysis, or management of software. Covers methods, models, assessment, and project management.
- **Fit for Graphcal:** Empirical evaluation of Git-friendly engineering calculations vs. spreadsheet workflows. Would need a strong empirical component.
- **Link:** https://ieeexplore.ieee.org/xpl/RecentIssue.jsp?punumber=32

#### JSS — Journal of Systems and Software (Elsevier)
- **Scope:** All aspects of software engineering. Welcomes empirical studies, tools, and the "New Ideas and Trends Paper" (NITP) track.
- **Fit for Graphcal:** Good for a tool/system paper with evaluation. The NITP track could work for the Graphcal vision.
- **Link:** https://www.sciencedirect.com/journal/journal-of-systems-and-software

#### EMSE — Empirical Software Engineering (Springer)
- **Scope:** Empirical insights into SE methodologies.
- **Fit for Graphcal:** User studies comparing Graphcal vs. spreadsheets for engineering calculations. Needs strong empirical evaluation.
- **Link:** https://www.springer.com/10664

---

## Tier 4: End-User Programming & Human-Centric Computing

For the "replacing spreadsheets" angle and making engineering calculations accessible.

### Conferences

#### VL/HCC — IEEE Symposium on Visual Languages and Human-Centric Computing
- **Scope:** Making programming accessible, understandable, and usable. Covers visual PLs, end-user programming, cognitive aspects of SE, programming by demonstration.
- **Fit for Graphcal:** Live view interaction, spreadsheet replacement, end-user programmability for engineers. VL/HCC has a strong history of spreadsheet research (e.g., "Understanding and Inferring Units in Spreadsheets" at VL/HCC 2020).
- **Frequency:** Annual (October). VL/HCC 2025: Raleigh, NC, Oct 7–10.
- **Link:** https://conf.researchr.org/home/vlhcc-2025

#### SPLASH-E — (at SPLASH)
- **Scope:** Education and programming. Co-located with SPLASH.
- **Fit for Graphcal:** Teaching engineering calculations with a proper programming language; transitioning from spreadsheets.

---

## Tier 5: Scientific & Engineering Computing

For the domain application — engineering calculations, dimensional analysis, simulation.

### Conferences

#### SciPy — Scientific Computing with Python
- **Scope:** Open-source Python tools for science. Tutorials, talks, sprints.
- **Fit for Graphcal:** Python interop story (Phase 9), comparison with NumPy/Pint for dimensional analysis. More of a practitioner venue than a research venue.
- **Frequency:** Annual (July). SciPy 2026: Minneapolis, July 13–19. EuroSciPy 2026: Krakow, July 18–23.
- **Link:** https://www.scipy2026.scipy.org

#### SIAM CSE — SIAM Conference on Computational Science and Engineering
- **Scope:** Computational science as a mode of scientific discovery alongside theory and experiment.
- **Fit for Graphcal:** Type-safe engineering computation, dimensional analysis for scientific software correctness. The next edition is CSE27 (2027, Pittsburgh). Call for Participation expected April 2026.
- **Link:** https://www.siam.org/conferences-events/siam-conferences/cse27/

#### SIAM PP — SIAM Conference on Parallel Processing for Scientific Computing
- **Scope:** Parallel and scalable scientific computation.
- **Fit for Graphcal:** Only if parallelism in DAG evaluation is developed. Niche fit.
- **Link:** https://www.siam.org/conferences-events/siam-conferences/pp26/

### Journals

#### SIAM Journal on Scientific Computing (SISC)
- **Scope:** Numerical methods and techniques for scientific computation. Papers must include computational results demonstrating effectiveness.
- **Fit for Graphcal:** A paper on compile-time dimensional analysis reducing scientific software errors, with empirical benchmarks. Niche — must show computational science value.
- **Link:** https://www.siam.org/publications/siam-journals/siam-journal-on-scientific-computing/

---

## Tier 6: Simulation & System Dynamics

For the system dynamics modeling capability (Vensim replacement).

### Conferences

#### ISDC — International System Dynamics Conference
- **Scope:** The annual conference of the System Dynamics Society. Methodology and application of system dynamics.
- **Fit for Graphcal:** "System dynamics as a pattern in a general-purpose reactive language" — comparison with Vensim, Stella, AnyLogic. Strong fit for Graphcal's system dynamics capability.
- **Frequency:** Annual.
- **Link:** https://systemdynamics.org/

#### WSC — Winter Simulation Conference
- **Scope:** Simulation modeling and analysis across domains. Has dedicated System Dynamics and Hybrid Modeling tracks.
- **Fit for Graphcal:** Graphcal as a language for hybrid simulation (SD + reactive computation). The SD track welcomes novel modeling approaches.
- **Frequency:** Annual (December). WSC 2026: Glasgow, December 6–9.
- **Link:** https://meetings.informs.org/wordpress/wsc2026/tracks/

### Journals

#### System Dynamics Review
- **Scope:** Refereed journal of the System Dynamics Society. Methodology and application of system dynamics.
- **Fit for Graphcal:** Paper on expressing SD models in Graphcal vs. Vensim — safety, composability, version control advantages.
- **Link:** Published by Wiley for the System Dynamics Society.

---

## Tier 7: Aerospace & Systems Engineering

Your home domain. These venues reach the target user community directly.

### Conferences

#### AIAA SciTech Forum
- **Scope:** Largest aerospace R&D event globally. Covers science, technologies, and policies shaping aerospace.
- **Fit for Graphcal:** "Type-safe engineering calculations for aerospace" — unit safety (Mars Climate Orbiter), scenario management for mission analysis. Best under Intelligent Systems or Multidisciplinary Design Optimization sessions.
- **Frequency:** Annual (January).
- **Link:** https://aiaa.org/events-learning/events/

#### AIAA AVIATION Forum
- **Scope:** Integrated spectrum of aviation R&D.
- **Fit for Graphcal:** Similar to SciTech but aviation-focused.
- **Frequency:** Annual (May). AVIATION 2026: May 19–21.
- **Link:** https://aiaa.org/events-learning/events/

#### IEEE Aerospace Conference
- **Scope:** Interdisciplinary understanding of aerospace systems, science, technology, and applications. Co-sponsored by AIAA and PHM Society.
- **Fit for Graphcal:** Aerospace software tools, model-based engineering for space missions.
- **Frequency:** Annual. 47th edition in 2026.
- **Link:** https://www.aeroconf.org/

#### INCOSE International Symposium
- **Scope:** Premier global event for systems engineering. Practitioners and researchers.
- **Fit for Graphcal:** Type-safe, collaborative engineering calculations for systems engineering workflows. MBSE alignment.
- **Frequency:** Annual (June/July). 2026: Yokohama, Japan, June 13–18.
- **Link:** https://www.incose.org/

#### CSER — Conference on Systems Engineering Research
- **Scope:** SE research, organized by INCOSE. Theme for 2026: "Intelligent Digital Twin-enabled Systems Engineering."
- **Fit for Graphcal:** Graphcal as a language for digital twin computations, integrating with MBSE workflows.
- **Dates:** April 6–9, 2026, at George Mason University, Arlington, VA.
- **Link:** https://sercuarc.org/event/incose-annual-conference-on-systems-engineering-research-2026/

#### MODELS — ACM/IEEE International Conference on Model-Driven Engineering Languages and Systems
- **Scope:** All aspects of modeling for software and systems — languages, methods, tools, applications.
- **Fit for Graphcal:** Graphcal as a modeling language for engineering systems. Comparison with UML/SysML approaches.
- **Frequency:** Annual (October). MODELS 2026: Malaga, Spain, October 4–9.
- **Link:** https://conf.researchr.org/home/models-2026

### Journals

#### AIAA Journal of Aerospace Information Systems (JAIS)
- **Scope:** Aerospace information and software systems.
- **Fit for Graphcal:** Tool paper on Graphcal for aerospace engineering workflows.
- **Link:** https://arc.aiaa.org/journal/jais

#### Systems Engineering (Wiley/INCOSE)
- **Scope:** Multidisciplinary SE for products, services, and processes.
- **Fit for Graphcal:** Graphcal for collaborative, version-controlled engineering calculations in SE practice.
- **Link:** https://www.incose.org/

---

## Recommended Publication Strategy

Based on Graphcal's contributions, here is a suggested multi-paper strategy organized by aspect:

### Paper 1: Language Design (Flagship Paper)
**Target:** OOPSLA or SLE (conference), then expanded to TOPLAS (journal)
**Focus:** The full Graphcal language design — motivation (Mars Climate Orbiter, spreadsheet hell), the 6-layer type system, reactive DAG model, Git-friendly design. Worked examples from aerospace engineering.
**Readiness:** After Phase 4 (MVP with multi-file support).

### Paper 2: Type System — Dimension Algebra & Space Safety
**Target:** ESOP, POPL, or PLDI (conference), then TOPLAS (journal)
**Focus:** Formal treatment of the dimension type system (extending Kennedy's dimension types), phantom-type-based space safety (extending Sguaba), and the orthogonal layering of these systems. Metatheoretic results (type safety, decidability).
**Readiness:** Requires formal metatheory work.

### Paper 3: Incremental Computation for Engineering Calculations
**Target:** OOPSLA or PLDI (conference)
**Focus:** The reactive DAG with hybrid eager/lazy evaluation, dirty tracking with early cutoff (backdating), durability classification. Empirical evaluation vs. full recomputation.
**Readiness:** After Phase 1–2 with benchmarks.

### Paper 4: Vision — Replacing Spreadsheets with a Programming Language
**Target:** Onward! Papers or Onward! Essays (at SPLASH)
**Focus:** The philosophical and practical case for moving engineering calculations from spreadsheets to a proper language. Git-friendliness, scenario management, type safety, unit awareness. Less rigorous validation required — compelling argument + worked examples suffice.
**Readiness:** Can be written now, even before full implementation.

### Paper 5: System Dynamics as a Language Pattern
**Target:** ISDC (System Dynamics Society) or WSC (System Dynamics track)
**Focus:** Expressing SD models in Graphcal vs. Vensim/Stella — `index`/`scan`/struct instead of stock/flow keywords. Advantages: type safety, composability, version control. Worked example: a classic SD model reimplemented in Graphcal.
**Readiness:** After Phase 7 (system dynamics).

### Paper 6: Tool/Experience Paper for Aerospace Audience
**Target:** AIAA SciTech or INCOSE International Symposium
**Focus:** Applied paper showing Graphcal solving real aerospace engineering problems — orbital mechanics, mass budgets, trade studies with scenarios. Emphasize unit safety, auditability, and team collaboration.
**Readiness:** After Phase 6+ with real-world examples.

### Paper 7: End-User Programming & Spreadsheet Replacement
**Target:** VL/HCC
**Focus:** User study comparing engineers using spreadsheets vs. Graphcal for a set of engineering tasks. Measure error rates (especially unit errors), collaboration friction, and auditability.
**Readiness:** Requires working prototype + user study.

### Paper 8: The Programming Journal (Broad Audience)
**Target:** The Art, Science, and Engineering of Programming
**Focus:** A "programming pearl" style paper on Graphcal's design — the elegance of phantom types for space safety, or how system dynamics emerges as a pattern. Accessible to a broad PL audience.
**Readiness:** Can target the "Art" perspective early.

---

## Quick Reference: Upcoming Deadlines (2026)

| Venue | Type | Deadline | Event Date |
|---|---|---|---|
| OOPSLA 2026 (Round 2) | Conf | Mar 17, 2026 | Oct 2026 (SPLASH) |
| CSER 2026 | Conf | (passed) | Apr 6–9, 2026 |
| ESOP 2026 | Conf | (passed) | Apr 2026 (ETAPS) |
| ICSE 2026 | Conf | (passed) | Apr 12–18, 2026 |
| AIAA AVIATION 2026 | Conf | TBD | May 19–21, 2026 |
| INCOSE IS 2026 | Conf | TBD | Jun 13–18, 2026 |
| PLDI 2026 | Conf | (passed) | Jun 15–19, 2026 |
| ECOOP 2026 | Conf | TBD | Jun 29 – Jul 3, 2026 |
| GPCE 2026 | Conf | TBD | Jun/Jul 2026 (w/ ECOOP) |
| SciPy 2026 | Conf | Feb 25, 2026 | Jul 13–19, 2026 |
| MODELS 2026 | Conf | TBD | Oct 4–9, 2026 |
| SPLASH/Onward! 2026 | Conf | TBD | Oct 2026 |
| SLE 2026 | Conf | TBD | Oct 2026 (w/ SPLASH) |
| VL/HCC 2026 | Conf | TBD | Oct 2026 |
| WSC 2026 | Conf | TBD | Dec 6–9, 2026 |
| Programming Journal | Journal | Rolling (next: Feb 1, 2026) | — |
| TOPLAS | Journal | Rolling | — |
| JFP | Journal | Rolling | — |
| SoSyM | Journal | Rolling | — |
| IEEE TSE | Journal | Rolling | — |
| SISC | Journal | Rolling | — |
| System Dynamics Review | Journal | Rolling | — |
