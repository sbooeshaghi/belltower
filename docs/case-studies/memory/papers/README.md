# Memory Papers And References

This index tracks paper and article references relevant to Belltower's memory
work, especially machine readability, claim extraction, and research artifact
structuring.

Mirrored PDFs live in `./pdfs/`.

## Machine Readability

- [Rethinking the production and publication of machine-readable expressions of research findings](https://www.nature.com/articles/s41597-025-04905-0)
  - Scientific Data, 2025
  - argues for producing born-readable research findings instead of relying only
    on post-publication extraction
  - local mirror: `pdfs/2025-scientific-data-machine-readable-expressions.pdf`
- [The Future of PubMed Central: Publicly Accessible, Digitally Equitable, Universally Valuable](https://nlmdirector.nlm.nih.gov/2024/10/23/the-future-of-pubmed-central-publicly-accessible-digitally-equitable-universally-valuable/)
  - NLM Musings from the Mezzanine, 2024
  - not a paper; useful as infrastructure and accessibility context for
    machine-readable publication pipelines
- [Building and Exploiting a Web of Machine-Readable Scientific Facts to Make Discoveries](https://ceur-ws.org/Vol-3643/paper1.pdf)
  - CEUR-WS workshop paper
  - focuses on nanopublications, semantic predications, and machine-readable
    scientific claims
  - local mirror: `pdfs/2024-ceur-machine-readable-scientific-facts.pdf`
- [ArXiParse](https://arxiparse.org/)
  - not a paper; tool/product reference
  - useful as a practical example of turning papers into structured,
    LLM-consumable objects, including claims and figure interpretation

## Claim Extraction

- [Towards Fine-Grained Extraction of Scientific Claims from Heterogeneous Tables Using Large Language Models](https://www.vldb.org/2025/Workshops/VLDB-Workshops-2025/TaDA/TaDA25_16.pdf)
  - VLDB Workshop, 2025
  - claim extraction from heterogeneous tables into structured
    subject-measure-outcome triples
  - local mirror: `pdfs/2025-vldb-tada-fine-grained-claim-extraction-tables.pdf`
- [Using Large Language Models for Hypotheses and Claims Extraction from Scientific Literature](https://dl.acm.org/doi/pdf/10.1145/3701716.3717752)
  - WWW Companion, 2025
  - ACM page was not directly fetchable from the current environment; title was
    resolved from the DOI metadata surfaced through an institutional index
  - link-only: no local mirror yet because direct PDF fetch is access-restricted
- [CLAIMCHECK: How Grounded are LLM Critiques of Scientific Papers?](https://aclanthology.org/2025.findings-emnlp.1185.pdf)
  - Findings of EMNLP, 2025
  - relevant for claim verification, critique grounding, and review-linked claim
    analysis rather than extraction alone
  - local mirror: `pdfs/2025-findings-emnlp-claimcheck.pdf`
- [Structured information extraction from scientific text with large language models](https://www.nature.com/articles/s41467-024-45563-x)
  - Nature Communications, 2024
  - structured extraction baseline/reference for scientific text using LLMs
  - local mirror: `pdfs/2024-nature-communications-structured-information-extraction.pdf`

## Mirror Status

Local mirrors currently present:

- `pdfs/2024-ceur-machine-readable-scientific-facts.pdf`
- `pdfs/2024-nature-communications-structured-information-extraction.pdf`
- `pdfs/2025-findings-emnlp-claimcheck.pdf`
- `pdfs/2025-scientific-data-machine-readable-expressions.pdf`
- `pdfs/2025-vldb-tada-fine-grained-claim-extraction-tables.pdf`

Link-only references:

- PubMed Central accessibility article
- ArXiParse
- ACM WWW Companion claim extraction paper

## Suggested Use In Belltower

- machine-readable publication work informs how Belltower should model durable,
  inspectable scientific artifacts instead of treating papers as opaque blobs
- claim extraction papers inform future `bt-memory` pipelines for extracting
  reusable semantic units from sessions and external literature
- tool references such as ArXiParse are useful for subsystem and product-boundary
  study even when they are not academic papers
