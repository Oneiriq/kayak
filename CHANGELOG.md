# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Kayak has not cut a release yet. Everything below is the road to 0.1.0.

## [Unreleased]

### Added

- **A resource can pin to one of several columns.** `pinned` is an
  AND, which cannot describe a symmetric relationship: a friendship
  stored as one row per unordered pair holds its accounts in `a` and
  `b`, the caller is either of them, and pinning one side would credit
  an index for half the rows. `pinned_either` says the server binds to
  one of a set.

  The index rule was measured against SurrealDB 3 rather than reasoned
  about. The engine answers the disjunction as a `UnionIndexScan`, one
  seek per branch, so every alternative must head an index whose
  remaining columns serve the filter and sort claims. A branch with no
  index behind it is refused, naming the branch. Changing the set is
  breaking both ways: narrowing hides rows, widening discloses them.

- **The dispatcher reports rather than panics, and kayak has no
  production `unwrap` left.** Eight sites relied on the build-time
  completeness gate having registered a resolver, and asserted it at
  request time. A panic there takes down every other in-flight request
  on the same task and tells an operator a line number, where a 500
  tells them which operation is unserved. Both now answer
  `KayakError::Internal` naming the operation, and say plainly that
  reaching it means the gate and the dispatcher disagree -- a kayak
  bug, not the caller's.

- **A resource with no face needs no fields.** Requiring an exposure
  on a resource that never renders its row type was requiring a
  projection nobody builds. Found writing polyconsole-social's
  contract, where `friends` is five two-account verbs over a table of
  unordered pairs and `me` is three writes that name no id: both are
  resources whose only job is to give their actions a path. A face
  still demands fields, and so does an action answering with the
  resource, because those do render it.

- **A resource says which collection faces it exposes.** Every resource
  emitted a listing and a getter, which misdescribes a resource whose
  collection is deliberately not browsable. polyconsole-social serves
  `GET /accounts/{id}` and must never serve `GET /accounts` — a social
  service does not enumerate its users — and it hangs a keys
  sub-resource off that same resource, so leaving `accounts` undeclared
  cost the sub-resource too. The other shape is a domain that is all
  verbs: `friends` is eight two-account RPCs over a table of unordered
  pairs, with no listing and no getter to declare.

  `Resource.faces` declares it, both by default and omitted from the
  serialized document when it is. Gated in all seven artifacts, because
  a gate that reached six of them would leave one still promising the
  enumeration the contract just refused. Actions and sub-resources are
  independent of both flags, which is what makes a faceless resource
  useful rather than empty.

  Turning the listing off refuses `filterable`, `sortable`,
  `filter_options` and `watchable` — each is a claim about an endpoint
  that does not exist, and kayak refuses claims that cannot be true
  rather than letting them reach the artifacts as promises. A resource
  exposing nothing at all is refused outright. Withdrawing a face is
  breaking; restoring one is not.

- **A contract says where its routes live.** `/v1` was hardcoded in
  four client generators and the OpenAPI emitter, which quietly made
  kayak a generator for services that had already chosen kayak's
  version prefix. A service serving `/accounts` could not adopt a
  generated client that called `/v1/accounts`, and moving its routes to
  suit the generator breaks whatever is already shipped against them —
  the wrong direction for a tool whose job is catching breaks.

  `Contract.api_prefix` declares it, `/v1` by default and omitted from
  the serialized document when it is, so existing contracts round-trip
  unchanged and every golden holds byte for byte. An empty prefix puts
  the resources at the root. Moving it is breaking in both directions,
  since every resource route moves at once. Queries are unaffected —
  they declare absolute paths, which is why a query could always
  describe a service's real routes when a resource could not.

- **A contract says how its callers authenticate.** Copal's
  `x-copal-tenant` header was hardcoded in four client generators, so a
  service authenticating any other way had four files to edit and no
  way to say so in the contract. `Contract::auth` now declares the
  scheme — `none`, `bearer`, or a named header — and every face reads
  the declaration: the four clients, the OpenAPI `securitySchemes` and
  document-level `security`, and the differ, which treats any change of
  scheme as breaking in both directions. The credential's *name* is
  part of the declaration and reaches the generated constructors, so a
  bearer contract gets `Client::new(url, token)` and copal's gets
  `Client::new(url, tenant)`. Declaring copal's existing scheme
  reproduces all four client goldens byte for byte, which is what
  establishes the mechanism is faithful to the behaviour it replaced.

- **A blocking Rust client, on request.** `--targets
  client-rs-blocking` emits `client_blocking.rs`: the same types,
  renames, nullability, auth scheme, URLs and query shaping as
  `client-rs`, reached through `reqwest::blocking`. A caller with no
  runtime no longer has to stand up an executor to make one call or
  hand-maintain a synchronous port that drifts from the contract.

  One generator parameterised by how a call suspends, not two to keep
  in step. The four fragments that differ — asyncness, await, the
  reqwest module, the feature list — are the whole of it, and a test
  asserts exactly that by converting the async output into the blocking
  one and demanding byte equality. Opt-in rather than default, like
  `engine-policy`: it is a second flavour of a language the default set
  already covers, and defaulting it would hand every consumer a second
  Rust client to review.

- **The vector-index gate: a declared search with nothing behind it
  does not generate.** The backing rules could refuse a search resting
  on the wrong machinery, but not a search resting on none — a query
  that declared no backing and a query that needed none were the same
  document, so the contract had no way to say "this performs a
  semantic search" and therefore no way to be wrong about it. That is
  the exact shape unindexed search has when it ships: driftnet's chunk
  search and antumbra's recall paths both ran for months over columns
  nothing could answer a neighbour query on, and no contract anywhere
  could have caught either, because nobody writes down the index they
  do not have.

  `Query.searches` is the declaration — a list of `SearchKind` — and
  every kind named there must have a backing of that kind behind it,
  or generation fails with `Violation::UnbackedSearch` naming the
  query and the kind. Losing a declared search is breaking; gaining
  one is compatible. The field is empty by default, old contracts
  deserialize and render unchanged, and a backing whose kind is not
  declared stays legal, so adoption is a line per query rather than a
  flag day. What the rule cannot do is make anyone declare, which is
  the standing limit of a declaration language and why `verify --db`
  exists beside it; what it buys is that a declaration, once made, is
  load-bearing.

- **A vector backing pins the width it searches at.**
  `SearchBacking.dimension` is held against the index's own
  `DIMENSION` (`Violation::BackingWidthMismatch`). A vector of the
  wrong width is not a slower search, it is a different one, and the
  width changes whenever the embedding model does — so a model swap
  that outran its schema is a generation failure rather than a quiet
  change in what comes back. `None` leaves the width to the
  deployment. A lexical backing has no width, and stating one there is
  refused rather than compared against a FULLTEXT index that was never
  going to have one.

- **A backing can be machinery the deployment configures.**
  `SearchBacking.optional` says the index may be absent. Copal is the
  case that forced it: its HNSW index over `text_chunk.embedding` is
  applied at startup, and only where an embedding model is configured,
  at that model's width — so declaring it outright would make the
  contract false everywhere else, and the answer until now was to
  declare nothing, which is the silence these rules exist to end.
  Optional relaxes exactly one check, index presence, and no others: a
  present index still holds the column, is the kind's own machinery,
  and matches the declared width. May be absent, never may be wrong.
  `verify --db` reads it the same way through
  `Expectation::ReachesIndexIfDefined` — the probe runs, and a plan
  that missed is excused on one fact, that this database does not
  define the index, established with one extra `INFO FOR TABLE` spent
  only when an optional backing already came back unserved.

  The differ matches backings by where the machinery is (table,
  column, index, kind) rather than member for member, so the width and
  the optional flag read as one change each: a width changed or
  dropped and a backing gone optional are breaking, a width newly
  pinned and a backing now required of every deployment are not. Under
  the old member-for-member comparison, strengthening a promise read
  as a loss and a gain, which is the kind of false alarm that teaches
  people to wave a gate through.

### Changed

- **Re-blessing a golden names the golden.** `KAYAK_BLESS` took any
  value and re-blessed all eight goldens at once, so reaching for it to
  read one generator's diff silently rewrote the other seven. It now
  takes the artifacts to bless — `KAYAK_BLESS=client-go`, a comma
  separated list, or `1`/`all` for the blanket form a change touching
  every face still wants. An unrecognised name fails rather than
  blessing nothing quietly: `golang` is a plausible thing to type when
  Go is `client-go` internally, and a bless that matched nothing would
  read as a clean run. A test asserts every `TARGETS` entry has a bless
  name, so a new target cannot arrive with no way to re-bless it alone.

- **One client generator per module.** `clients.rs` reached 1,245 lines
  and a second Rust flavour was about to make that worse. Split to
  `clients/{rust,typescript,python,go}.rs`, with only what more than
  one language needs left in `clients/mod.rs`. A pure move: the
  goldens do not shift.

- **Generated Rust is checked.** The CLI test compiled the generated
  Python and parsed the generated Go, but nothing looked at the
  generated Rust at all. It now runs `rustfmt` over both flavours,
  which parses before it formats. That catches syntax only — a stray
  `.await` in the blocking client parses fine and fails to build — so
  the generators suite separately asserts the blocking flavour never
  suspends.

- **`tests/search_gate.rs` carries the search half of the gate.**
  `contract_gate.rs` crossed a thousand lines. What moved is already
  one subject asked from two sides — a filter or a sort resting on
  search machinery, and a search resting on a b-tree — so it went out
  whole rather than being trimmed, and both directions still read
  against the same `text_chunk` fixture.

- **A vector backing rests on DISKANN the way it rests on HNSW.**
  surql 0.33 added the DISKANN index kind, and it answers KNN through
  the same `<|k,EF|>` operator the verify probe composes, so
  `serves_search` accepts it for a `SearchKind::Vector` backing and
  the refusal message names all three machineries. A lexical claim on
  one is refused as before.


### Added

- **Declared search: a query names the machinery that answers it.**
  A listing declares its cost exhaustively — every filter and sort
  claim index-validated — while search, the one read whose cost is
  most surprising, was an opaque `Query`: typed inputs, a path, and
  nothing about what serves them. Copal's real search is the proof:
  BM25 over `text_chunk.body` through `idx_chunk_body`, HNSW over
  `text_chunk.embedding` through `idx_chunk_embedding`, fused in the
  resolver, and all of it invisible to validation, to the differ, and
  to `verify --db`, so nothing stopped a schema change from dropping
  `idx_chunk_body` while the contract went on promising search.

  `Query.backing` is the declaration: each `SearchBacking` names one
  column of one table reached through one index of a stated kind,
  `lexical` (`@@` through FULLTEXT) or `vector` (KNN through HNSW or
  MTREE). A fused search is two backings on one query; the fusion is
  resolver behavior, not contract. The field is optional and empty by
  default, so plain queries stay legal and old contracts deserialize
  unchanged — `ir_revision` stays at 1, because the revision marks
  changes an older reader would misread and an absent backing means
  today exactly what its absence meant before.

  Validation holds a backing to the mirror image of the listing index
  rules, through the same `indexes.rs` predicate module, which now
  answers the type question in both directions. A backing resting on
  a plain b-tree is refused the way a filter resting on a FULLTEXT
  index is: `Violation::WrongBackingIndexType` names the index and
  what it turned out to be, and says what each kind needs. Unknown
  table, column, and index each refuse by name, and an index that
  resolves but holds a different column is its own refusal rather
  than a type complaint about the wrong thing.

  The differ reads a removed backing, or ANY member of one re-pointed
  (table, column, index, kind), as breaking — a backing has no name
  of its own, so its identity is its four members — and a backing
  added to an existing query as compatible: it promises more about
  the same wire surface. The completeness prover's fixture carries a
  backing and mutates every member separately, so the coverage is
  proven rather than assumed.

  The backing is capacity metadata, not wire shape. The SDL and all
  four clients are byte-identical with or without one (held by test);
  the declaration surfaces where metadata already rides — the MCP
  tool's annotations beside scope and rate, and the OpenAPI operation
  description. `verify --db` probes each backing through its own
  operator and holds the plan to the NAMED index, which is stricter
  than not scanning, because a search served by some other index than
  the declared one is drift too. The plan vocabulary is probed, not
  guessed: on SurrealDB 3.x a served `@@` answers `FullTextScan` and
  a served `<|k,EF|>` answers `KnnScan`, each naming its index in the
  leaf's attributes; unserved, both degrade to `TableScan`, and the
  metric KNN form plans `KnnTopK` over a `TableScan` even where an
  index exists, which is why the probe composes `<|k,EF|>` the way
  copal does. The probe literal is `[0]` whatever the embedding
  dimension, because the planner resolves the index before it looks
  at the literal's width. One boundary is the engine's, stated
  rather than papered over: SurrealDB 3.x has removed MTREE (the
  `DEFINE` no longer parses; `<|k|>` errors "no longer supported"),
  so a vector backing on an MTREE-typed definition validates
  statically but cannot exist on a live 3.x database, and
  verification composes only the HNSW form.

  The scaffold is explicitly unchanged: it writes no queries, so it
  writes no backings.

### Fixed

- **A pinned column set could reach no index at all, and validation
  said nothing.** Pins are the one predicate with nothing optional
  about it: filterable describes what a caller MAY send and sortable
  what they may ask for, but the bound columns ride every read the
  resource serves. The gate checked that each pin existed and asked
  no more, so a tenant-scoped resource over a table whose every index
  serves someone else validated clean, and its plain listing, nothing
  filtered and nothing sorted, scanned the table with the pins as its
  only predicate. The same shape as the index-type fix below, through
  the other door: that one caught claims a caller might exercise,
  this one catches the query the contract compels.

  The rule belongs to the set, not to any single pin.
  `Violation::UnreachableListing` fires when no standard or unique
  index leads with a bound column; one leading bound column is
  enough, because the engine seeks its range and checks the remaining
  pins inside it. That is what keeps copal's deliveries sub-collection
  legal, and it is why the rule credits a sub-resource's `parent_key`:
  a delivery log indexed by endpoint and never by tenant is cheap
  reached through the endpoint and a scan reached directly, and the
  violation distinguishes the two reaches rather than the table.

  The scaffold now declines such a table entirely and names it on
  stderr beside the withheld secret columns, for the same reason it
  withholds them: exposing it without the pins would publish across
  the boundary the pins draw, exposing it with them writes a resource
  the validator refuses, and the missing index is a schema change a
  contract tool does not get to make. Against copal's 25 tables the
  scaffold now exposes 22, and the three it declines (`file_version`,
  `tus_upload`, `webhook_delivery`) are exactly the tables copal's
  hand-written contract never lists at the top level. The shared
  predicate lives in `indexes.rs` with the ordering-index rule, read
  by both the scaffold and the validator, so the two halves cannot
  drift on this either.

- **A filter or a sort could rest on an index that cannot serve it.**
  The gate asked whether a claimed column appeared in any index on the
  table and stopped there. SurrealDB spells five kinds of index with
  one `DEFINE INDEX`, and three of them have no b-tree behind them:
  FULLTEXT answers `@@` against an analyzer's terms, HNSW and MTREE
  answer nearest-neighbour over a vector, and none of the three
  narrows an equality or supplies an order. A column covered only by
  one of them read as covered, so an equality filter on a BM25-indexed
  body passed validation and scanned the table, and an `ORDER BY` down
  an HNSW index passed validation and is not a thing the engine will
  do. That is the exact failure kayak exists to prevent, admitted by
  the thing that prevents it.

  The scaffold had the rule right all along and filtered on index
  type, which is what made this hard to see. The half that reads a
  schema and writes claims was careful; the half that reads claims a
  person wrote was not, and a person is who writes the contracts. The
  scaffold's own test asserted only that it declined to claim a
  full-text column, never that a hand-written claim on one was
  refused, so the strict rule never ran on human input. There is one
  predicate now and both halves read it, because two copies of a rule
  is how they came to disagree.

  `Violation::WrongIndexType` is the refusal, and it names the index
  rather than denying that one exists. An author looking straight at
  `DEFINE INDEX idx_chunk_body ... FULLTEXT` and told the column was
  "not covered by any index" goes hunting for the bug in kayak; the
  message now reads "filterable column body is indexed on text_chunk,
  but only by the FULLTEXT index idx_chunk_body, which serves neither
  an equality filter nor an ORDER BY". A column a standard index does
  hold, behind an unbound prefix, keeps the older prefix violation,
  since naming the index type there would send the author off to
  define an index they already have. A column carrying both kinds is
  the ordinary way to make one searchable and filterable at once, and
  is accepted as it always was.

  Copal's checked-in contract is unaffected. Its only two non-ordering
  indexes sit on `text_chunk`, and no resource or sub-resource targets
  that table: it is reached through queries, which declare no filter
  or sort claims.

### Added

- **`kayak verify --db`: ask the planner itself.** Static validation
  proves an index exists for every filter and sort claim; it cannot
  prove the planner uses it. An index can cover the right columns in
  an order the composed listing cannot seek, and an engine upgrade
  can re-cost a plan overnight — every such case ships a listing that
  answers correctly and walks the table to do it, which is the exact
  failure the static gate exists to prevent, one layer below where it
  can see. `verify` composes one representative listing per filter
  claim and per sort claim (pins as equality binds, the claimed
  filter bound, the claimed sort ordered, always with a LIMIT), runs
  each through `EXPLAIN` against a live database, and fails naming
  the claim whenever the plan iterates the table. Library API
  (`kayak::verify::{probes, verify_contract}`) and CLI, exiting
  non-zero so it gates in CI beside `diff`.

  The plan vocabulary is probed, not guessed: on SurrealDB 3.x an
  `EXPLAIN` answers with one plan tree of `operator` nodes, index
  access spells `IndexScan`, the fallback spells `TableScan`, a sort
  the index order cannot serve rides a `SortTopKByKey` node over
  whichever scan feeds it, and a filter the index cannot narrow
  becomes a residual `Filter` over the pins' seek — so a `TableScan`
  anywhere in the tree is the conviction. The probe lives on as a
  vocabulary test against the embedded engine, because this tool's
  worst failure mode is the vocabulary drifting under it and turning
  every verification silently green.

  Kayak deliberately carries no database client, so the whole module
  rides a new `verify` cargo feature the way async-graphql rides
  `graphql`: the client arrives only for consumers that opt in, CI
  runs `--all-features` so the gated half stays compiled and tested,
  and the tests drive the in-process `mem://` engine so no server is
  required anywhere.

- **The engine policy: the eighth face, derived instead of hand-kept.**
  A SurrealDB deployment can enforce the contract a second time at
  the engine — table `PERMISSIONS` filtering rows, field
  `PERMISSIONS` redacting columns — for sessions authenticated as
  callers rather than as the service. The clauses worth having are
  exactly what the contract declares, and copal derived them by hand
  in its server, which is the drift this library exists to prevent:
  tighten a scope in the contract, forget to re-derive, and the API
  refuses what the engine still serves, with nothing naming the
  divergence. `kayak::derive_policy` is that derivation moved home.
  `reads_require` becomes a select conjunct on the resource's table
  and every sub-resource table, since a sub-collection is read under
  its parent's requirement; a field guard becomes a column redaction,
  at both nesting levels.

  The token-claim vocabulary the clauses speak (which claim carries
  the scope list, what clause a named guard becomes) is deployment
  convention rather than contract content, so it travels as a
  `ClaimVocabulary` argument whose defaults are copal's conventions —
  the reference deployment's switch to this API is proven a
  behavioral no-op in `tests/policy.rs`, byte-for-byte against what
  its hand derivation renders, with the matched copal sources cited.
  A guard the vocabulary cannot render refuses the derivation naming
  the guard, because rendering nothing would silently drop the engine
  layer for a column the application layer kept enforcing.

  Two rules stay with the service deliberately. The mechanical
  tenancy floor derives from the SCHEMA, not the contract: a floor
  derived from the contract would be dodgeable by omission, and a
  future table left out of the contract must land under the floor,
  not above it. And delete conjuncts (retention) are policy the
  contract cannot declare yet, so they are the service's to state
  explicitly rather than this module's to invent.

  The CLI gained an opt-in `engine-policy` target rendering
  `policy.json` beside the other artifacts, so the engine's row
  security is review-visible in the same commit that moves it. Opt-in
  rather than default because the CLI holds only the default
  vocabulary; a contract naming its own guards would fail the whole
  default run over a face nobody asked for.

- **A reference page: the contract, as the surface it becomes.** A
  service declares its shape once and kayak lands it on REST,
  GraphQL, and MCP by rules nobody should have to hold in their head.
  The OpenAPI document gives the REST half, the SDL gives the GraphQL
  half, and until now nothing put them side by side.

  For every operation the contract declares, the page shows the path a
  REST caller takes, the field a GraphQL caller selects, the tool an
  agent calls, the scopes it requires, and its rate class. Each
  resource carries what it hands back, including a field's underlying
  column when it is renamed and a guard when one applies, and how a
  caller may narrow it, including a filter's own option list and the
  page-size ceiling. It reads the same functions the generators read,
  so it cannot drift from the documents.

  Every operation also carries the request itself: the REST call with
  its body, the GraphQL document with its arguments and selection, and
  the MCP tool call. A mapping alone answers where an operation lives,
  and the next question is always what to send. Enum fields note their
  alternatives so a sample value does not read as a default, an action
  answering the resource carries a selection while one answering JSON
  does not, and an instance action names the id it takes as an
  argument rather than only in a path. All twenty-three documents
  copal's contract produces parse and validate against copal's own
  generated schema.

  "Try it" on each row lands on the control that runs it, with an
  action's dialog already open.

  Beneath the requests, every input is stated rather than inferred:
  its declared type, whether it is required, whether it rides in the
  path, the query string, or the body, and the values it accepts when
  those are a closed set. A sample value lets a reader guess that
  `"width": 0` is an integer and that a missing field was optional,
  and guessing is not the same as being told. The type is said in the
  contract's own vocabulary, since one call reaches three faces and
  `String!` belongs to GraphQL alone. An instance action's id appears
  there too, marked as riding in the path, because it is required
  without being declared among the inputs.

- **A footer**, carrying the contract's name and version, so a page
  ends rather than stops.

- **`document`, `Page`, and `rail_section` are exported**, so a host
  renders its own pages in the same frame. A second implementation of
  the frame is a second console: copal's deployment page kept its own
  markup and stayed on the old layout while every generated page
  moved, which is the failure the stylesheet already had before it
  was shared. The generated pages go through the same function.

### Changed

- **Removing a field's guard is breaking now, and a sub-collection's
  guards answer to the differ at all.** The differ read guards
  caller-side only: adding or swapping one broke, removing one
  "showed more and refused nobody" and passed as compatible, and a
  guard on a sub-resource field was not compared in any direction —
  which is exactly where copal's one guarded field lives. Who sees a
  field is contract surface in BOTH directions: removing a guard
  takes away the redaction itself, showing the column to every caller
  the guard used to deny, on the API faces and now in the derived
  engine `PERMISSIONS` too. So every guard movement is named breaking
  and review decides, at both nesting levels, through one shared
  field-diff so the rules cannot drift by depth. Sub-resource fields
  also gained the column-retarget check (same wire name over a
  different column) and additive field reports the top level already
  had.

- **The console has a shape.** Navigation moved into a rail down the
  left and the data fills the rest, which is what an operator already
  knows from every console they use and leaves the whole width for the
  thing they came to look at. The rail marks where they are.

- **Actions are things you can do, rather than forms nobody asked to
  see.** Every action's form used to lie open below the data, so a
  file with six of them buried its own record under a wall of inputs.
  Each action is now a button in a toolbar beside the heading, and its
  form arrives in a `<dialog>` when asked for: the browser already
  knows about the backdrop, the escape key, and where the focus goes.

- **Declared names are said the way people write them.** A contract
  names things for machines, and printing `content_type` and
  `issue_url` raw is what made the console read as a dump of the IR.
  Columns, labels, headings, and navigation are sentence case now,
  with the acronyms an operator would never see lowercased.

  A control also says what it will do. "Create" alone names nothing,
  so a collection action takes the thing it acts on and reads "Create
  file"; the submit inside the dialog says the same rather than
  "perform". An instance action already has its subject on the page
  and would only repeat it.

- **The appearance control is a mark rather than a word**, since
  switching light and dark says itself faster as a half-lit circle
  than as the word "theme" set beside the navigation.

- **A submit stands apart from the fields above it**, on its own
  footer with a rule, instead of butting against the last input.

### Added

- **Several of a set.** `ActionField.multiple` says a caller may name
  more than one of `options`. The value travels as one comma-separated
  string, which is what a query parameter carries without ceremony,
  and every part answers to the set rather than the joined whole. The
  console renders checkboxes, since a menu that allows several hides
  that it does and wants a modifier key to work; the OpenAPI document
  and the MCP manifest render an array of the enum. `validate` refuses
  `multiple` on a field that lists no options, which would be several
  of nothing.

- **A light theme, and a control that overrides it.** Daylight arrives
  on its own under `prefers-color-scheme: light`, since an operator on
  a bright screen reading a black page is the same problem as the
  reverse. A `theme` control in the header cycles system, light, dark,
  and remembers the choice, because following the machine is a good
  default rather than an answer for everyone.

  The whole sheet moved onto tokens to make this cheap: every color
  reads a custom property, so a theme is one block of tokens and
  nothing else in the sheet needs a second version. The control writes
  `data-theme` on the root, which beats the media query in both
  directions, and the two lines that apply a stored choice sit in the
  head so a chosen theme never flashes the other one. All of it is
  additive: with scripting off, the automatic behavior is exactly
  what it was.

### Changed

- **The overview says what is here.** Each resource card read "8
  actions, 1 sub-collections", which describes the contract and
  answers a question nobody opening a console has. A card now lists
  the resource's first page: how many rows, whether more follow, and a
  few of them.

  Which column names a row is decided by asking which one tells the
  rows apart, since a generated console knows nothing about the
  domain. Preferring a name-like column alone put "file.ready" on four
  event cards; asking for the most distinct column alone put four raw
  timestamps there. So a preview carries both, a label and a time,
  because what an operator wants from a card is what the row is and
  when. The time drops its year and seconds, which a card has no room
  for and the listing page still carries.

  A resource whose listing refuses says so on its own card and leaves
  the rest of the page standing.

### Added

- **Closed sets are declared, enforced, and offered as menus.** An
  input whose values are a fixed list said so in prose and nowhere a
  machine could read: the console gave a text box, the OpenAPI
  document promised a string, and a caller learned the vocabulary by
  guessing or by reading a description. `ActionField.options` and
  `Resource.filter_options` declare the list.

  It reaches every face. The OpenAPI document and the MCP manifest
  carry it as `enum`, the console renders a menu in place of a box,
  and the dispatcher refuses a value outside the list ahead of the
  resolver, so the declaration cannot drift into a promise nothing
  keeps. The differ reads a narrowing set as breaking, including the
  introduction of a set where anything used to pass, and a widening
  one as compatible.

  `validate` refuses filter options that name a column outside
  `filterable`, an empty list, options on a field that cannot hold a
  string, and a value listed twice.

### Fixed

- **The narrow button answered 400 when no sort was chosen.** The sort
  control offers "declared order" as an empty value, and the query
  parser read that as a request to sort on a column named `""`, which
  the dispatcher refused. Every other control already ignored an empty
  value; sort and cursor now do too.

### Changed

- **The console reads at a glance.** A listing is scanned rather than
  read, and the raw projection defeated scanning. A nested object
  printed as a hundred and twenty characters of JSON wrapped down
  twenty lines in a narrow column and took the row with it, so five
  records made a page three thousand pixels tall. Digests ran to
  sixty-four characters, timestamps to nanoseconds, byte counts to
  whatever integer the row held.

  Values now render for what they turn out to be. Objects and arrays
  collapse to `{3 fields}` or `[2]` and open on a click. Digests cut
  to a stub, timestamps lose their sub-seconds, byte counts read in
  units, and every shortening keeps the exact value in `title`, so the
  table never becomes the reason a value cannot be read. A state gets
  a chip colored by a small fixed vocabulary; a word outside it stays
  neutral, because color is the one thing an operator trusts without
  reading. Access levels stay neutral for the same reason turned
  around: green reads as healthy, and `public` is the most exposed a
  record gets.

  The page around them changed to match. Wide tables scroll in their
  own box so the body never does, headers stick, headings and labels
  take a UI face while data keeps the monospace that makes columns of
  ids line up, and action forms sit in a grid rather than a stack.

- **`STYLE` and `cell` are exported.** A host renders pages of its own
  beside the generated ones, and two stylesheets means two consoles.
  Copal's deployment page was the case: it kept a copy and went on
  printing raw byte counts and nanosecond timestamps after the
  generated pages stopped.

### Fixed

- **The generated clients carry queries.** `Contract.queries` reached
  OpenAPI, the SDL, the MCP manifest, the console, and both runtime
  routers; the four client generators never read it. A service could
  declare a search, publish it on every other face, and hand out an
  SDK with no way to call it. Copal was in that position: `search` and
  `file_text` were absent from all four clients.

  Each query now emits a method taking the path parameter, if the path
  spells one, and its remaining inputs as named typed parameters, the
  way listings already read. The answer is the language's open JSON
  type, because the IR declares no shape for a query and inventing a
  struct would invent a promise. Required parameters are ordered ahead
  of optional ones, since TypeScript and Python both refuse the other
  order. A query with nothing to encode builds no query string, which
  also keeps the Rust binding free of an unused `mut`.

  The Rust client encodes through reqwest rather than joining pairs by
  hand, because a search term carries spaces and ampersands. Both
  query shapes are in the shared test fixtures now, so the goldens
  record them and the CLI test's real Python and Go toolchains parse
  what the generators emit.

### Added

- **`kayak scaffold`, for services that already have a database.**
  Kayak refused to do anything without a contract, and writing the
  first one meant transcribing every column and checking every filter
  and sort against an index by hand, which is the step that stopped
  adoption before it started. `scaffold --schema` reads the schema and
  writes a contract that validates against it, so the first `generate`
  produces artifacts instead of a list of claims to repair. Against
  copal's real 25 tables it derives 47 filters and 33 sorts, and all
  seven targets generate from the result unedited.

  It claims less than it could, deliberately. Sorts are claimed only
  where pinned columns cover the index prefix ahead of them, which is
  stricter than `validate` accepts. `filterable` describes what a
  caller may send, nothing obliges them to send it, and an unfiltered
  sort down a composite index scans. The differ calls a removed sort breaking and
  an added one compatible, so an invented claim costs a major version
  to withdraw while an omitted one costs a line. Columns whose names
  suggest a secret are left unexposed and reported, on the same
  reasoning applied to a worse outcome.

- **Content faces.** A resource may declare `content: { upload,
  download }`, and the byte paths render into the OpenAPI document
  as octet-stream operations (`PUT/GET /v1/{resource}/{id}/content`)
  under the differ's governance: removing a face is breaking. Bytes
  are streams rather than JSON, so these stay off the GraphQL and
  MCP shapes; the grant actions remain the agent path to the same
  content.

### Added

- **The MCP tool manifest.** `generate_mcp_tools` derives an MCP
  `tools/list` document from the contract: every resource's list and
  get, every action, and every query becomes a tool with a JSON
  Schema input derived from the same declarations, and scope and
  rate classes ride each tool as annotations. The manifest joins
  `generate_all` as the `mcp` target, so the agent surface is
  governed by the same drift gate and differ as every other face.
  `serde_json` now pins `preserve_order` unconditionally, so
  generated artifacts are byte-stable across feature sets.

### Changed

- **Field guards see the row.** A guard now receives
  `Option<&serde_json::Value>` beside the context: `Some(row)` when
  projecting, `None` when the question precedes rows (filter and
  sort narrowing). Ownership guards become expressible: show a
  caller their own rows' values while hiding the rest, within one
  listing, on every face. The rowless form still gates narrowing,
  because a caller who only partially sees a column must not filter
  by it. `guarded_fields` and `strip_guarded` join the shared
  projection API so hand-written faces project row-by-row exactly as
  the dispatcher does.

### Added

- **Contract queries: reads that answer a question.** A listing
  cannot express a search, because relevance is not a sort column and
  a query string is not a filter. `Contract.queries` declares named
  reads with typed parameters; they render as GraphQL query fields
  and REST `GET`s, dispatch through the same chain every other
  operation uses, and carry the same scope and rate declarations. The
  answer is JSON, since its shape belongs to the resolver rather than
  to a projected table. Resolvers are gated in both directions: a
  declared query without one refuses to build, and a resolver for a
  query nobody declared refuses too. The differ treats them like
  actions, so removing one, renaming its field, moving its path,
  tightening its scopes, or gaining a required parameter all read as
  breaking. Queries also render into OpenAPI as `GET` operations,
  with declared parameters split between path and query string, so
  the REST face of a query is published rather than merely served.

### Added

- **Per-principal watch ceilings.** `limits.max_watches_per_principal` caps
  concurrently open subscriptions per caller; the slot is taken before the
  resolver runs, so a refused open never starts a live query, and it rides
  the stream so dropping the subscription frees it. Over the ceiling is
  `too_many_requests`: closing a subscription is what frees a slot. Carried
  in `x-limits`; introducing or lowering the ceiling diffs as breaking.
- **The projection API for hand-written faces.** `hidden_fields`,
  `hidden_in`, and `strip_hidden` expose the dispatcher's own guard
  computation, so a service's hand-written REST handlers redact from the
  same declarations the GraphQL face enforces instead of drifting apart.
  The dispatcher delegates to the shared core, so the two cannot diverge.
- **Field guards as dispatcher projection.** A named visibility policy on an
  exposed field, registered by the service and applied once in the dispatcher
  on every row a resolver returns, subscriptions included. Denied fields are
  omitted; reads are never errors; a guarded field renders nullable on every
  generated surface and carries `x-guard` in OpenAPI. A caller who cannot see
  a column cannot filter or sort by it, because narrowing by a value is
  reading it. The completeness gate runs both directions, and the differ
  treats guarding an open field or swapping its policy as breaking.
- **Rate classes and the dispatch limiter.** A contract defines named budgets
  (`rate_classes`) and attaches them to resource reads or individual actions.
  The dispatcher charges the ledger before anything else runs: listings cost
  their clamped row limit, everything else costs one, buckets key on the
  principal's subject, and exhaustion is `too_many_requests`. A metered
  contract refuses to build without a registered `RateStore`; the in-memory
  store covers one process and a shared implementation covers a fleet. The
  differ treats new metering or a shrunken budget as breaking.
- **Principal and scopes.** A `Principal` (subject plus scopes) rides the
  context; `reads_require` on a resource gates list, get, sub-collections,
  and watching, and `requires` on an action gates invoking it. Enforcement
  happens once, in the dispatcher after the middleware chain and before the
  resolver, so every face inherits it identically. Anonymous against a
  declared scope is `unauthorized`; identified but missing it is `forbidden`,
  naming the scope. OpenAPI operations carry `x-requires-scopes`, and the
  differ treats a new requirement as breaking.
- **Consumption refusals in the error vocabulary.** `PayloadTooLarge` (413)
  and `TooManyRequests` (429), so a protocol face no longer downgrades an
  oversized body to a generic bad request and a metered refusal has a status a
  client can wait on.
- **Contract-declared limits.** Depth and complexity ceilings live in the
  contract, where the served GraphQL schema applies them before any resolver
  runs, the OpenAPI document carries them as `x-limits`, and the differ treats
  introducing or lowering one as a named breaking change.
- **Sub-resources.** A collection belonging to one parent instance (a file's
  versions, an endpoint's deliveries) is declared on the parent and reaches
  every face from that one declaration: `GET /v1/files/{id}/versions`, a field
  on the parent's GraphQL type, an OpenAPI path, and a method on all four
  clients. The index rulebook is shared with resources, with `parent_key`
  credited as equality-bound, and the GraphQL type name composes with the
  parent so two parents may each carry a `versions` collection.
- **Watchable resources and the subscription seam.** A resource may declare
  itself `watchable`, which adds a GraphQL Subscription field over a stream
  resolver the service registers. REST, OpenAPI, and the four generated clients
  are untouched: Kayak generates no long-lived HTTP operations. Declaration and
  registration are checked in both directions at build, so a watchable resource
  without a resolver refuses by name, and so does a resolver for a resource the
  contract never opened. Watchers narrow the stream with the same `filterable`
  columns list callers use. The middleware chain runs once, at open.
- **The contract, executed.** The runtime: a resolver registry a service fills
  with its own data access, a middleware chain that is protocol-agnostic, and a
  dispatcher that enforces the contract before any resolver runs. Construction
  refuses, by name, any declared operation without a resolver.
- **The live GraphQL face.** An `async-graphql` dynamic schema built from the
  same IR the SDL generator prints, so the served schema and the checked-in
  artifact cannot disagree. `schema_builder` exposes the underlying builder for
  depth and complexity limits.
- **Generators for every surface**: OpenAPI 3.1, GraphQL SDL, and clients in
  Rust, TypeScript, Python, and Go, all from one contract object.
- **Actions**: verbs beyond list and get, with typed inputs and instance or
  collection targeting, rendered as OpenAPI operations, GraphQL mutations, and
  client methods from one declaration.
- **Validation against the real schema.** Tables and columns must exist,
  renames must not collide, filters must be indexed, and sorts must be
  reachable through an index prefix whose head is equality-bound. A sort no
  index can serve is a generation error naming the column.
- **Name gating.** Chosen names are checked against the GraphQL grammar, the
  `__` introspection prefix, root type names, cross-resource type collisions,
  and the SurrealDB v3 reserved-word list, exported as `kayak::is_reserved`.
- **IR-level diffing.** `kayak diff old.json new.json` classifies every change
  and exits non-zero on a breaking one, which makes the gate one line of CI.

### Fixed

- **OpenAPI list responses are the page envelope**, matching the SDL and the
  generated clients, rather than a bare array.
