//! The contract intermediate representation.
//!
//! A contract is data: serializable, versioned, diffable, checked in.
//! It never restates the database schema: resources
//! reference tables and columns by name, and validation resolves those
//! references against the authoritative `surql-rs` definitions. What
//! lives here is exclusively API-side: exposure, renaming, filter and
//! sort allowlists, and pagination bounds.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A complete API contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contract {
    /// Contract name; becomes the OpenAPI title.
    pub name: String,
    /// Semantic version of the contract itself (not the service).
    pub version: String,
    /// IR schema revision, for forward-compatible tooling.
    #[serde(default = "default_ir_revision")]
    pub ir_revision: u32,
    /// Named consumption budgets. A resource or action that names one
    /// is metered against it; introducing or shrinking a budget is a
    /// breaking change the differ names. Defined here so the budgets
    /// are review-visible beside the operations they bound.
    #[serde(default)]
    pub rate_classes: Vec<RateClass>,
    /// Request-cost ceilings, declared here so they appear in the
    /// artifacts and the differ tracks them. A ceiling hand-wired at
    /// the protocol layer is policy the contract does not know about:
    /// invisible in review, silent when it tightens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ContractLimits>,
    /// The path every resource face hangs under, `/v1` by default.
    ///
    /// It was hardcoded, which made kayak a generator for services
    /// that had chosen kayak's version prefix before they had kayak.
    /// A service already serving `/accounts` cannot adopt a client
    /// that calls `/v1/accounts`, and telling it to move its routes
    /// breaks whatever is already shipped against them -- for a
    /// contract layer whose point is catching breaks, that is the
    /// wrong direction to push.
    ///
    /// Empty means the resources sit at the root. Queries are
    /// unaffected either way: they declare absolute paths, which is
    /// why they could always describe a service's real routes.
    #[serde(default = "Contract::default_api_prefix")]
    #[serde(skip_serializing_if = "Contract::is_default_api_prefix")]
    pub api_prefix: String,
    /// How a caller proves who it is.
    ///
    /// Every generated client has to put a credential on the wire, and
    /// until this existed each of them hardcoded one consumer's
    /// convention: `x-copal-tenant`, in eight places across four
    /// languages. That made kayak a generator of clients for copal
    /// rather than for contracts -- a service authenticating with a
    /// bearer token got a client that sent somebody else's header and
    /// no credential at all.
    ///
    /// It belongs in the contract for the same reason scopes and rate
    /// classes do: it is part of what the API promises its callers,
    /// the differ should notice when it changes, and the OpenAPI
    /// document should say it out loud rather than leaving a reader to
    /// infer it from an example. [`AuthScheme::None`] by default, so a
    /// contract that says nothing sends nothing.
    #[serde(default, skip_serializing_if = "AuthScheme::is_none")]
    pub auth: AuthScheme,
    /// Exposed resources.
    pub resources: Vec<Resource>,
    /// Reads that are not listings: a question with typed inputs and
    /// an answer, ordered by whatever the resolver decides. Search is
    /// the shape that motivated them, and a listing cannot express
    /// one: relevance is not a sort column and a query string is not
    /// a filter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queries: Vec<Query>,
}

/// How a caller proves who it is, and therefore what every generated
/// client puts on the wire.
///
/// Three narrow cases rather than a general
/// security-scheme vocabulary. Each one is something a generated
/// client can actually DO without asking the caller to write transport
/// code: put a fixed header on, or send nothing. OAuth flows, signed
/// requests and mTLS are all real, and none of them is a header a
/// generator can fill in from a constructor argument, so they belong
/// to the service rather than here. Widen this when a consumer needs
/// it, not in anticipation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AuthScheme {
    /// No credential. The client sends nothing and its constructor
    /// takes only a base URL.
    #[default]
    None,
    /// `Authorization: Bearer <token>`. The constructor takes a token.
    Bearer,
    /// An opaque value in a named header -- copal's `x-copal-tenant` is
    /// the case this generalises. The constructor takes a value named
    /// after the credential rather than after the header.
    Header {
        /// The header name, sent verbatim.
        name: String,
        /// What the credential is called in the generated constructor
        /// and field (`tenant`, `api_key`). Purely cosmetic, and worth
        /// having: `Client::new(url, tenant)` reads like the service it
        /// talks to, where `Client::new(url, credential)` reads like a
        /// generator.
        #[serde(default = "AuthScheme::default_credential_name")]
        credential: String,
    },
}

impl AuthScheme {
    /// Whether the contract declares no credential. Used to keep the
    /// field out of a rendered contract that never set it, so existing
    /// documents stay byte-identical.
    #[must_use]
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// What the credential is called when a header scheme does not say.
    fn default_credential_name() -> String {
        "credential".to_owned()
    }

    /// The credential's name in generated code, or `None` when there is
    /// no credential to name.
    #[must_use]
    pub fn credential_name(&self) -> Option<&str> {
        match self {
            Self::None => Option::None,
            Self::Bearer => Some("token"),
            Self::Header { credential, .. } => Some(credential),
        }
    }

    /// The header a client sets, and the value expression's prefix.
    /// `None` when the scheme sends no header.
    #[must_use]
    pub fn header(&self) -> Option<(&str, &'static str)> {
        self.wire().map(|wire| (wire.header, wire.prefix))
    }

    /// Everything a generated client needs to put a credential on the
    /// wire, or `None` for an open API.
    ///
    /// One value rather than [`Self::header`] and
    /// [`Self::credential_name`] read separately. Those are `Some`
    /// together and `None` together, but nothing in their types said
    /// so, and each of the four generators paid for it the same way:
    /// matching on one and unwrapping the other with a note explaining
    /// why it could not fail. Four proofs of an invariant, none of them
    /// checked. Here the invariant is the shape.
    #[must_use]
    pub fn wire(&self) -> Option<AuthWire<'_>> {
        match self {
            Self::None => Option::None,
            Self::Bearer => Some(AuthWire {
                header: "authorization",
                prefix: "Bearer ",
                credential: "token",
            }),
            Self::Header { name, credential } => Some(AuthWire {
                header: name,
                prefix: "",
                credential,
            }),
        }
    }
}

/// What a generated client puts on the wire to authenticate.
///
/// Borrowed from the scheme rather than owned, so producing it costs
/// nothing and no generator is tempted to cache it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthWire<'a> {
    /// The header name, sent verbatim.
    pub header: &'a str,
    /// What precedes the credential in the header value, empty when
    /// the credential is sent bare.
    pub prefix: &'static str,
    /// What the credential is called in generated constructors and
    /// fields.
    pub credential: &'a str,
}

/// One named read that answers a question rather than paging a
/// collection.
///
/// Queries render as GraphQL query fields and REST `GET`s, carry the
/// same scope and rate declarations resources and actions carry, and
/// the differ treats them the way it treats actions: adding one is
/// additive, removing or renaming one breaks, and tightening what it
/// requires breaks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Query {
    /// Snake-case name; generators derive per-language method and
    /// GraphQL field names from it.
    pub name: String,
    /// REST path, absolute under the API root (`"/v1/search"`).
    pub path: String,
    /// Parameters, which arrive as query-string values on REST and
    /// as field arguments on GraphQL.
    #[serde(default)]
    pub input: Vec<ActionField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// GraphQL field name override (default: camelCase of the name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphql_field: Option<String>,
    /// Scopes a caller must hold. Empty means open.
    #[serde(default)]
    pub requires: Vec<String>,
    /// The rate class metering this query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_class: Option<String>,
    /// The search machinery this query performs.
    ///
    /// A backing says what answers a search; this says the search
    /// happens at all, and validation joins the two: a kind declared
    /// here with no backing of that kind behind it is a promise of
    /// indexed search over nothing indexed, and is refused. Without
    /// this field that promise had no way to be made, so it had no way
    /// to be broken -- a query that declared no backing was
    /// indistinguishable from a query that needed none, which is
    /// exactly how a semantic search over an unindexed column ships
    /// and goes unnoticed. Empty means the query searches nothing,
    /// which is what every query written before this field said and
    /// goes on saying.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub searches: Vec<SearchKind>,
    /// What answers the search, when the query is one. A listing
    /// declares its cost exhaustively -- every filter and sort claim is
    /// index-validated -- while a query, the one read whose cost is
    /// most surprising, was an opaque box: typed inputs, a path, and
    /// nothing about the machinery behind it. So nothing stopped a
    /// schema change from dropping the FULLTEXT index while the
    /// contract went on promising search. Each backing names one
    /// column of one table reached through one index of a stated
    /// kind, and validation holds the index to the same standard the
    /// listing rules hold theirs to. A fused search (copal's: BM25
    /// candidates and HNSW neighbors, rescored together) is two
    /// backings on one query; the fusion itself is resolver behavior,
    /// not contract. Empty means the query claims no search machinery,
    /// which is what every existing contract declares.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backing: Vec<SearchBacking>,
}

/// One thing a search query's answer rests on: a column of a table,
/// reached through a named index of a stated kind.
///
/// A backing has no name, so where the machinery is -- table, column,
/// index, kind -- is its identity, and re-pointing any of the four
/// changes what the query promises about the same wire surface. The
/// width and the optional flag are not identity but what the backing
/// promises about that machinery, and the differ reads the two halves
/// differently for exactly that reason. `verify --db` probes the
/// whole thing: the named index must be the one the planner reaches
/// for the kind's operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchBacking {
    /// The table whose rows the search selects from.
    pub table: String,
    /// The column the search reads.
    pub column: String,
    /// The index that answers the search operator over that column.
    pub index: String,
    /// Which operator the index answers.
    pub kind: SearchKind,
    /// The width a vector search sends, held against the index's own
    /// `DIMENSION`.
    ///
    /// A vector of the wrong width is not a slower search, it is a
    /// different one, and the width changes whenever the embedding
    /// model does -- so pinning it here turns a model swap that outran
    /// its schema into a generation failure instead of a quiet change
    /// in what comes back. `None` leaves the width to the deployment,
    /// which is the honest declaration when the index is applied at
    /// startup at a configured width rather than written into the
    /// static schema. A lexical backing has no width, and stating one
    /// there is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimension: Option<u32>,
    /// Machinery the deployment is free not to provide.
    ///
    /// A static contract cannot claim an index that exists only where
    /// an operator configured one: copal's HNSW index over
    /// `text_chunk.embedding` is applied at startup, and only when an
    /// embedding model is configured, so declaring it outright would
    /// make the contract false in every deployment without one. The
    /// answer to that was to declare nothing, which is the silence
    /// this whole rulebook exists to end. Declared optional, absence
    /// stops being a lie -- the index may be missing, and a present one
    /// still has to hold the column, be the kind's own machinery, and
    /// match the declared width. Optional means may be absent, never
    /// may be wrong.
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
}

impl SearchBacking {
    /// Where the machinery is: what makes two backings the same
    /// backing, as against what they each promise about it.
    pub fn machinery(&self) -> (&str, &str, &str, SearchKind) {
        (&self.table, &self.column, &self.index, self.kind)
    }
}

/// A flag stays out of the rendered contract until it is set, so
/// adding one leaves every existing document byte for byte itself.
fn is_false(flag: &bool) -> bool {
    !*flag
}

/// The two kinds of search machinery an index can be.
///
/// The vocabulary is the contract's rather than the
/// engine's: `lexical` requires a FULLTEXT index (the `@@` operator),
/// `vector` an HNSW, MTREE, or DISKANN one (the KNN operator), and
/// validation translates between the two vocabularies when it
/// refuses. Which of the three vector machineries answers is the
/// schema's business, not the contract's -- the contract asks for
/// nearest neighbors through an index and the engine chooses how, so
/// moving a column from HNSW to DISKANN is a capacity decision the
/// contract does not have to be rewritten for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    /// Term matching over analyzed text: `@@` through FULLTEXT.
    Lexical,
    /// Nearest-neighbor over a stored vector: KNN through HNSW,
    /// MTREE, or DISKANN.
    Vector,
}

impl SearchKind {
    /// The word the contract uses, for violations and diff messages.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Vector => "vector",
        }
    }
}

impl std::fmt::Display for SearchKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Query {
    /// The GraphQL field name: the override, or camelCase of the
    /// query name (`file_text` -> `fileText`).
    pub fn graphql_field_name(&self) -> String {
        self.graphql_field
            .clone()
            .unwrap_or_else(|| crate::naming::camel(&self.name))
    }
}

/// One named consumption budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateClass {
    /// Snake-case name resources and actions reference.
    pub name: String,
    /// Units allowed per caller per minute. A listing costs its row
    /// limit; every other operation costs one.
    pub units_per_minute: u64,
}

/// Ceilings on what one operation may cost. The served GraphQL schema
/// enforces both before any resolver runs; REST operations have fixed
/// shape and depth, so the ceilings exist for the face where cost is
/// caller-controlled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractLimits {
    /// Maximum selection depth of one GraphQL operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    /// Maximum field-selection count of one GraphQL operation, which
    /// is what alias amplification multiplies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_complexity: Option<u32>,
    /// Maximum concurrently open subscriptions per principal. A
    /// subscription holds server resources for as long as the client
    /// stays; without a ceiling, one caller can hold every live query
    /// the deployment will ever serve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_watches_per_principal: Option<u32>,
}

pub(crate) fn default_ir_revision() -> u32 {
    1
}

impl Contract {
    /// The prefix a contract gets when it does not say: what every
    /// resource face hung under before the prefix was declarable.
    #[must_use]
    pub fn default_api_prefix() -> String {
        "/v1".to_owned()
    }

    fn is_default_api_prefix(prefix: &str) -> bool {
        prefix == "/v1"
    }

    /// The prefix, normalized for joining: either empty or leading
    /// slash with no trailing one, so `{prefix}/{resource}` is right
    /// in both cases and no generator has to think about it.
    #[must_use]
    pub fn prefix(&self) -> &str {
        self.api_prefix.trim_end_matches('/')
    }
}

/// Which collection faces a resource exposes.
///
/// Both, before this existed. That suits a resource whose collection is
/// browsable, and misdescribes one whose is not: a social service
/// serves `GET /accounts/{id}` and must never serve `GET /accounts`,
/// because enumerating every user is the thing it is careful not to do.
/// A contract had no way to say that, so it either promised an endpoint
/// the service refuses to build or left the resource -- and everything
/// hanging off it -- undeclared.
///
/// Actions and sub-resources are independent of both flags. A resource
/// with neither face is still a place for verbs to live, which is what
/// an RPC-shaped domain like `friends` actually is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceFaces {
    /// `GET /{resource}`: the paged collection.
    #[serde(default = "ResourceFaces::yes")]
    pub list: bool,
    /// `GET /{resource}/{id}`: one instance by id.
    #[serde(default = "ResourceFaces::yes")]
    pub get: bool,
}

impl Default for ResourceFaces {
    fn default() -> Self {
        Self::ALL
    }
}

impl ResourceFaces {
    /// A browsable collection: both faces, which is what every
    /// resource had before it could say otherwise.
    pub const ALL: Self = Self {
        list: true,
        get: true,
    };
    /// Reachable by id, never enumerable.
    pub const GET_ONLY: Self = Self {
        list: false,
        get: true,
    };
    /// Enumerable, with no by-id face.
    pub const LIST_ONLY: Self = Self {
        list: true,
        get: false,
    };
    /// Neither: a resource that exists to carry actions and
    /// sub-resources.
    pub const NONE: Self = Self {
        list: false,
        get: false,
    };

    fn yes() -> bool {
        true
    }

    /// Whether this is the default pair, so it stays out of a
    /// serialized contract that never chose.
    fn is_default(&self) -> bool {
        *self == Self::ALL
    }

    /// Whether the resource exposes no collection face at all.
    #[must_use]
    pub fn is_none(&self) -> bool {
        !self.list && !self.get
    }
}

/// How a resource's instances are named on the wire.
///
/// Three states, because the wire has three: the SurrealDB-conventional
/// `id`, a domain column (`user`, `key`), or NO identity field at all --
/// a service that strips record ids and keys rows by their content. The
/// third state exists because every SurrealDB record has an id, so
/// whether the wire carries one is a serialization choice kayak cannot
/// infer; synthesising it anyway is how eight artifacts came to declare
/// a field one service never sends.
///
/// The serde form keeps every existing contract meaning what it meant:
/// absent = `Id`, a string = `Column`, an explicit `null` = `Absent`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Identity {
    /// The conventional `id` -- what saying nothing always meant.
    #[default]
    Id,
    /// A named column of the backing table.
    Column(String),
    /// No identity field on the wire at all. Instances cannot be
    /// addressed, so this refuses a get face, content faces, and
    /// id-taking actions at validation.
    Absent,
}

impl Identity {
    /// The column synthesised into the wire types, or `None` when the
    /// wire carries no identity. Every schema-and-struct emitter asks
    /// this.
    #[must_use]
    pub fn wire_column(&self) -> Option<&str> {
        match self {
            Identity::Id => Some("id"),
            Identity::Column(name) => Some(name),
            Identity::Absent => None,
        }
    }

    /// For serde: the default state serializes as nothing at all.
    fn is_default(&self) -> bool {
        matches!(self, Identity::Id)
    }
}

impl serde::Serialize for Identity {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            // Reached only through a container that ignores
            // skip_serializing_if; the honest spelling either way.
            Identity::Id => serializer.serialize_str("id"),
            Identity::Column(name) => serializer.serialize_str(name),
            Identity::Absent => serializer.serialize_none(),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Identity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // A string names the column; an explicit null says the wire has
        // no identity; an absent field never reaches here (serde's
        // `default` answers `Id` first).
        let named = Option::<String>::deserialize(deserializer)?;
        Ok(match named {
            Some(name) if name == "id" => Identity::Id,
            Some(name) => Identity::Column(name),
            None => Identity::Absent,
        })
    }
}

/// One exposed resource over one table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    /// API-facing name (plural, kebab/snake as the API prefers).
    pub name: String,
    /// Backing table in the schema.
    pub table: String,
    /// The column that names one instance on the wire: what the
    /// by-id path binds and what every emitted type carries as its
    /// identity field. [`Identity::Id`] is what every resource had
    /// before it could say otherwise.
    ///
    /// IT IS NOT ALWAYS `id`, AND ASSUMING SO EMITS A FIELD THE
    /// SERVICE DOES NOT SEND. A presence row is one per account and
    /// names itself `user`; a friendship keyed by an ordered pair
    /// names itself `key`. A client generated against the assumption
    /// fails to deserialize a successful response -- not at the
    /// margins, but on every read of that resource -- and the
    /// document, the SDL and four SDKs all carry the same mistake,
    /// because they all ask the same question of the IR.
    ///
    /// AND IT IS SOMETIMES NOTHING AT ALL. An invite on this wire is
    /// `{from, to, token, created_at}` -- no field names one row,
    /// because no caller ever addresses one (the resource is
    /// list-only, and its actions take the whole shape). Every
    /// SurrealDB record HAS an id, so kayak cannot infer this: whether
    /// the wire carries it is the service's serialization choice, and
    /// only the author knows it. [`Identity::Absent`] says it; the
    /// default keeps synthesising `id`, which every contract written
    /// before this could say so meant by saying nothing.
    ///
    /// Validated like any other column when named; a resource with no
    /// identity cannot expose a by-instance face, content faces, or an
    /// id-taking action, because there is nothing to address one by.
    #[serde(default, skip_serializing_if = "Identity::is_default")]
    pub identity: Identity,
    /// Projected fields. Nothing is exposed that is not listed.
    pub fields: Vec<FieldExposure>,
    /// Columns the SERVER always equality-binds before any caller input
    /// (tenant scoping, soft-delete filters). Never exposed as API
    /// parameters; they exist so index-prefix validation can credit
    /// them: an index `(tenant_id, state, created_at)` serves a
    /// `created_at` sort because `tenant_id` is pinned and `state` is
    /// filterable.
    #[serde(default)]
    pub pinned: Vec<String>,
    /// Columns the server binds the caller's value to ONE of, rather
    /// than all of.
    ///
    /// [`Self::pinned`] is an AND: every column is equality-bound, and
    /// the index rules credit them as a prefix. A symmetric
    /// relationship cannot be said that way. A friendship stored as
    /// one row per unordered pair holds the two accounts in `a` and
    /// `b`, and the caller is EITHER of them -- so their friendships
    /// are `a = me OR b = me`, and pinning `a` alone would be a claim
    /// the server does not honour, crediting an index for half the
    /// rows.
    ///
    /// The index rule is the prefix rule applied once per alternative:
    /// the listing is reachable only if EVERY named column heads an
    /// index whose remaining columns serve the filter and sort claims.
    /// That is what the engine needs -- it answers the disjunction as
    /// a union of one seek per branch, and a branch with no index
    /// behind it drags the whole read back to a scan.
    ///
    /// Fewer than two columns is refused: one alternative is a pin,
    /// and saying it this way only hides that.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_either: Vec<String>,
    /// Columns callers may filter on. Validated against indexes.
    #[serde(default)]
    pub filterable: Vec<String>,
    /// The values a filterable column accepts, for the columns whose
    /// values are a closed set. Keyed by column name; a column absent
    /// here takes anything. Same reasoning as an action field's
    /// `options`, applied to the other place a caller supplies a
    /// value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub filter_options: BTreeMap<String, Vec<String>>,
    /// Columns callers may sort on. Validated against index prefixes.
    #[serde(default)]
    pub sortable: Vec<String>,
    /// Page-size ceiling for list endpoints.
    #[serde(default = "default_max_page_size")]
    pub max_page_size: u32,
    /// Verbs beyond list/get: uploads, grants, deletions, workflow
    /// starts. An action binds to a domain use-case on the server; the
    /// contract only describes its wire shape.
    #[serde(default)]
    pub actions: Vec<Action>,
    /// The resource's byte faces, when it has content: an upload
    /// path, a download path, or both. Bytes are streams rather than
    /// JSON, so these render into the OpenAPI document as
    /// octet-stream operations instead of becoming GraphQL or MCP
    /// shapes; the grant actions are the agent path to the same
    /// bytes. Declaring them here puts the write half of a service
    /// under the differ: removing a face is breaking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<ContentFaces>,
    /// Collections that hang off ONE instance of this resource: a
    /// file's versions, an endpoint's deliveries. They read like a
    /// resource and are reachable only through a parent id, which is
    /// why they are not resources of their own.
    #[serde(default)]
    pub sub_resources: Vec<SubResource>,
    /// Which collection faces this resource exposes. Both by default,
    /// so a contract that says nothing behaves as it always did.
    #[serde(default, skip_serializing_if = "ResourceFaces::is_default")]
    pub faces: ResourceFaces,
    /// The rate class metering reads of this resource: list, get,
    /// sub-collections, and watch opens. Absent means unmetered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_class: Option<String>,
    /// Scopes a caller must hold to READ this resource: list, get,
    /// sub-collections, and watching all check them. Empty means open
    /// to any caller the middleware admits, which is every existing
    /// contract's behavior. A sub-collection is read under its
    /// parent's requirement, because it is reached through the parent.
    #[serde(default)]
    pub reads_require: Vec<String>,
    /// Whether callers may watch this resource for changes. A watchable
    /// resource gains a GraphQL Subscription field and requires a watch
    /// resolver; it changes nothing about REST, which has no long-lived
    /// shape here. Watchers narrow the stream with the same `filterable`
    /// columns list callers use, so a resource has ONE filter vocabulary
    /// whichever operation reads it.
    #[serde(default)]
    pub watchable: bool,
    /// GraphQL-scoped name overrides. REST paths and generated clients
    /// never see these; they exist because GraphQL names are part of a
    /// deployed schema's identity (fragments name types, queries name
    /// fields) and sometimes must differ from the derived defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphql: Option<GraphqlNames>,
}

/// GraphQL-side name overrides for one resource. Every member is
/// optional; absent members fall back to the derived names.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphqlNames {
    /// Object type name (default: PascalCase singular of the resource).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    /// Query field returning a page (default: camelCase resource name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_field: Option<String>,
    /// Query field returning one instance (default: camelCase singular).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub get_field: Option<String>,
    /// Subscription field delivering rows as they change (default:
    /// camelCase singular + `Changed`). Read only when the resource is
    /// watchable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_field: Option<String>,
}

/// A collection belonging to one instance of a parent resource.
///
/// It lists and pages like a resource, but it has no id-addressable
/// form of its own and no actions: everything about it is reached
/// through the parent. `GET /v1/files/{id}/versions` on REST, a field
/// on the parent object type in GraphQL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubResource {
    /// API-facing name, plural. Becomes the path segment and the
    /// GraphQL field on the parent.
    pub name: String,
    /// Backing table in the schema.
    pub table: String,
    /// Column on `table` holding the parent's id. The server always
    /// equality-binds it, so index validation credits it the way it
    /// credits `pinned`.
    pub parent_key: String,
    /// How this sub-resource's rows name themselves on the wire; the
    /// same three states as [`Resource::identity`], for the same
    /// reason -- an account's published keys are `{user, key_id,
    /// pubkey}`, and synthesising an `id` declares a field the
    /// service never sends.
    #[serde(default, skip_serializing_if = "Identity::is_default")]
    pub identity: Identity,
    /// Projected fields. Nothing is exposed that is not listed.
    pub fields: Vec<FieldExposure>,
    /// Further server-bound columns, beyond `parent_key`.
    #[serde(default)]
    pub pinned: Vec<String>,
    /// Columns callers may filter on. Validated against indexes.
    #[serde(default)]
    pub filterable: Vec<String>,
    /// Columns callers may sort on. Validated against index prefixes.
    #[serde(default)]
    pub sortable: Vec<String>,
    /// Page-size ceiling.
    #[serde(default = "default_max_page_size")]
    pub max_page_size: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// GraphQL-scoped name overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphql: Option<SubGraphqlNames>,
}

/// GraphQL name overrides for one sub-resource.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SubGraphqlNames {
    /// Object type name (default: parent singular + sub singular, e.g.
    /// `FileVersion`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    /// Field on the parent object (default: camelCase sub name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl SubResource {
    /// The GraphQL object type name: the override, or PascalCase
    /// singular of the parent and of this name (`files` + `versions`
    /// -> `FileVersion`). Composing with the parent is what keeps two
    /// parents with a `versions` collection from colliding.
    pub fn graphql_type_name(&self, parent: &Resource) -> String {
        self.graphql
            .as_ref()
            .and_then(|g| g.type_name.clone())
            .unwrap_or_else(|| {
                format!(
                    "{}{}",
                    crate::naming::type_name(&parent.name),
                    crate::naming::type_name(&self.name),
                )
            })
    }

    /// The field on the parent object type: the override, or camelCase
    /// of this name (`versions`).
    pub fn graphql_field(&self) -> String {
        self.graphql
            .as_ref()
            .and_then(|g| g.field.clone())
            .unwrap_or_else(|| crate::naming::camel(&self.name))
    }

    /// Columns the server equality-binds before any caller input:
    /// `parent_key` always, plus anything explicitly pinned.
    pub fn bound_columns(&self) -> Vec<&str> {
        std::iter::once(self.parent_key.as_str())
            .chain(self.pinned.iter().map(String::as_str))
            .collect()
    }
}

/// A resource's byte faces.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentFaces {
    /// `PUT /v1/{resource}/{id}/content` accepts the bytes.
    #[serde(default)]
    pub upload: bool,
    /// `GET /v1/{resource}/{id}/content` serves them.
    #[serde(default)]
    pub download: bool,
}

/// One verb on a resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    /// Snake-case action name; generators derive per-language method
    /// and mutation names from it.
    pub name: String,
    /// HTTP method: POST, PUT, DELETE, or PATCH.
    pub method: String,
    /// Path suffix under the resource (`"/{id}/url"`, `"/{id}"`, or
    /// `""` for the collection itself). A literal `{id}` marks an
    /// instance action and becomes a required id parameter everywhere.
    #[serde(default)]
    pub path: String,
    /// Request-body fields.
    #[serde(default)]
    pub input: Vec<ActionField>,
    /// What the action returns.
    #[serde(default)]
    pub output: ActionOutput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// GraphQL mutation field name override (default: camelCase
    /// singular resource + PascalCase action, e.g. `fileIssueUrl`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphql_field: Option<String>,
    /// Scopes a caller must hold to invoke this action. Empty means
    /// open, which is every existing contract's behavior.
    #[serde(default)]
    pub requires: Vec<String>,
    /// The rate class metering this action. Absent means unmetered;
    /// there is no fallback to the resource's class, because an
    /// action's cost profile rarely matches its resource's reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_class: Option<String>,
}

impl Action {
    /// Whether this action targets one instance (path carries `{id}`).
    pub fn takes_id(&self) -> bool {
        self.path.contains("{id}")
    }

    /// The GraphQL mutation field name: the override, or camelCase
    /// singular resource + PascalCase action (`fileIssueUrl`).
    pub fn graphql_field_name(&self, resource: &Resource) -> String {
        self.graphql_field.clone().unwrap_or_else(|| {
            format!(
                "{}{}",
                crate::naming::camel(&crate::naming::singular(&resource.name)),
                crate::naming::pascal(&self.name),
            )
        })
    }
}

impl Resource {
    /// The column that names one instance: [`Self::identity`], or
    /// `id`, or `None` when the wire carries no identity at all.
    ///
    /// Every emitter asks this rather than writing `"id"`, so a
    /// resource whose rows are keyed by something else -- or by
    /// nothing -- describes itself the same way in the document, the
    /// SDL, the MCP tools and all four SDKs. Contexts that ADDRESS an
    /// instance (the get path, content faces) cannot render without
    /// one; validation refuses those combinations, so an emitter
    /// reaching a `None` there reports it as the internal
    /// inconsistency it is rather than inventing a field.
    #[must_use]
    pub fn wire_identity(&self) -> Option<&str> {
        self.identity.wire_column()
    }

    /// The GraphQL object type name: the override, or PascalCase
    /// singular of the resource name (`files` -> `File`).
    pub fn graphql_type_name(&self) -> String {
        self.graphql
            .as_ref()
            .and_then(|g| g.type_name.clone())
            .unwrap_or_else(|| crate::naming::type_name(&self.name))
    }

    /// The Query field returning a page: the override, or camelCase
    /// resource name (`files`).
    pub fn graphql_list_field(&self) -> String {
        self.graphql
            .as_ref()
            .and_then(|g| g.list_field.clone())
            .unwrap_or_else(|| crate::naming::camel(&self.name))
    }

    /// The Query field returning one instance: the override, or
    /// camelCase singular (`file`).
    pub fn graphql_get_field(&self) -> String {
        self.graphql
            .as_ref()
            .and_then(|g| g.get_field.clone())
            .unwrap_or_else(|| crate::naming::camel(&crate::naming::singular(&self.name)))
    }

    /// The Subscription field delivering rows as they change: the
    /// override, or camelCase singular + `Changed` (`fileChanged`).
    pub fn graphql_watch_field(&self) -> String {
        self.graphql
            .as_ref()
            .and_then(|g| g.watch_field.clone())
            .unwrap_or_else(|| {
                format!(
                    "{}Changed",
                    crate::naming::camel(&crate::naming::singular(&self.name)),
                )
            })
    }
}

/// One request-body field of an action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionField {
    pub name: String,
    pub kind: TypeRef,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the caller may name several of `options` at once.
    ///
    /// The value travels as one comma-separated string, which is what
    /// a query parameter can carry without ceremony. Meaningless
    /// without `options`, since a set is what there is to choose
    /// several of.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multiple: bool,
    /// The values this input accepts, when they are a closed set.
    ///
    /// Empty means anything the type allows. A non-empty list is a
    /// constraint the dispatcher enforces, so it cannot drift from
    /// what the resolver does: a value outside it is refused before
    /// the resolver runs. It also reaches every face, as an `enum` in
    /// the OpenAPI document and the MCP manifest, and as a menu in the
    /// console instead of a box a caller has to guess into.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// Wire types for action inputs, kept small; anything richer
/// is `Json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeRef {
    String,
    Int,
    Bool,
    Json,
}

/// What an action returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutput {
    /// The parent resource's object schema.
    Resource,
    /// A free-form JSON object.
    #[default]
    Json,
    /// Nothing (HTTP 204).
    None,
}

fn default_max_page_size() -> u32 {
    100
}

/// One projected column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldExposure {
    /// Column name in the table.
    pub column: String,
    /// API-facing name; defaults to the column name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<String>,
    /// Named guard deciding, per caller, whether this field is
    /// visible. A guarded field renders nullable on every generated
    /// surface and is OMITTED from rows the guard denies; a read is
    /// never an error. Absent means visible to every caller the
    /// operation admits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<String>,
}

impl FieldExposure {
    /// Expose a column under its own name.
    pub fn column(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            rename: None,
            guard: None,
        }
    }

    /// Expose a column under an API-facing name.
    pub fn renamed(column: impl Into<String>, rename: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            rename: Some(rename.into()),
            guard: None,
        }
    }

    /// Guard this exposure with the named policy.
    pub fn with_guard(mut self, guard: impl Into<String>) -> Self {
        self.guard = Some(guard.into());
        self
    }

    /// The name the API surface uses.
    pub fn api_name(&self) -> &str {
        self.rename.as_deref().unwrap_or(&self.column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_round_trips_as_data() {
        let contract = Contract {
            name: "copal".into(),
            version: "1.0.0".into(),
            ir_revision: 1,
            api_prefix: "/v1".into(),
            limits: None,
            rate_classes: vec![],
            auth: Default::default(),
            resources: vec![Resource {
                name: "files".into(),
                table: "file".into(),
                identity: Default::default(),
                fields: vec![
                    FieldExposure::column("path"),
                    FieldExposure::renamed("size_bytes", "size"),
                ],
                pinned: vec!["tenant_id".into()],
                pinned_either: vec![],
                filterable: vec!["state".into()],
                sortable: vec!["created_at".into()],
                max_page_size: 100,
                actions: vec![Action {
                    name: "issue_url".into(),
                    method: "POST".into(),
                    path: "/{id}/url".into(),
                    input: vec![ActionField {
                        name: "ttl_secs".into(),
                        kind: TypeRef::Int,
                        required: false,
                        multiple: false,
                        description: None,
                        options: Vec::new(),
                    }],
                    output: ActionOutput::Json,
                    description: Some("Issue a signed URL.".into()),
                    graphql_field: None,
                    requires: vec![],
                    rate_class: None,
                }],
                graphql: None,
                watchable: false,
                reads_require: vec![],
                rate_class: None,
                sub_resources: vec![],
                faces: Default::default(),
                content: None,
                filter_options: Default::default(),
            }],
            queries: vec![Query {
                name: "search".into(),
                path: "/v1/search".into(),
                input: vec![],
                description: None,
                graphql_field: None,
                requires: vec![],
                rate_class: None,
                searches: vec![SearchKind::Lexical, SearchKind::Vector],
                backing: vec![
                    SearchBacking {
                        table: "text_chunk".into(),
                        column: "body".into(),
                        index: "idx_chunk_body".into(),
                        kind: SearchKind::Lexical,
                        dimension: None,
                        optional: false,
                    },
                    SearchBacking {
                        table: "text_chunk".into(),
                        column: "embedding".into(),
                        index: "idx_chunk_embedding".into(),
                        kind: SearchKind::Vector,
                        dimension: Some(768),
                        optional: true,
                    },
                ],
            }],
        };
        let json = serde_json::to_string_pretty(&contract).unwrap();
        let back: Contract = serde_json::from_str(&json).unwrap();
        assert_eq!(back, contract);
        assert_eq!(back.resources[0].fields[1].api_name(), "size");
        assert_eq!(back.queries[0].backing[0].kind, SearchKind::Lexical);
        assert_eq!(back.queries[0].backing[1].dimension, Some(768));
        assert!(back.queries[0].backing[1].optional);
    }

    /// Contracts written before backings existed deserialize unchanged,
    /// and a query that declares none serializes without the key: the
    /// field is invisible in both directions unless something is
    /// declared, which is why `ir_revision` stays at 1 -- the revision
    /// marks changes an older reader would MISREAD, and an absent
    /// `backing` means today exactly what its absence meant before.
    ///
    /// The same holds for everything the gate has added since: a
    /// declared search, a pinned width, an optional backing. Each is
    /// absent by default and skipped when absent, so a contract that
    /// says nothing about search renders identically to the day it was
    /// written.
    #[test]
    fn a_contract_without_backings_is_the_contract_it_always_was() {
        let old = r#"{
            "name": "copal",
            "version": "1.0.0",
            "resources": [],
            "queries": [{
                "name": "search",
                "path": "/v1/search"
            }]
        }"#;
        let contract: Contract = serde_json::from_str(old).unwrap();
        assert_eq!(contract.ir_revision, 1);
        assert_eq!(contract.queries[0].backing, vec![]);
        assert_eq!(contract.queries[0].searches, vec![]);
        let rendered = serde_json::to_string(&contract).unwrap();
        for key in ["backing", "searches", "dimension", "optional"] {
            assert!(!rendered.contains(key), "{key} in {rendered}");
        }
    }

    /// A backing written before the width and the optional flag
    /// existed still reads, and reads as what it always meant: a width
    /// the contract does not pin, and machinery the deployment is
    /// required to have.
    #[test]
    fn a_backing_without_a_width_is_the_backing_it_always_was() {
        let old = r#"{
            "name": "copal",
            "version": "1.0.0",
            "resources": [],
            "queries": [{
                "name": "search",
                "path": "/v1/search",
                "backing": [{
                    "table": "text_chunk",
                    "column": "body",
                    "index": "idx_chunk_body",
                    "kind": "lexical"
                }]
            }]
        }"#;
        let contract: Contract = serde_json::from_str(old).unwrap();
        let backing = &contract.queries[0].backing[0];
        assert_eq!(backing.dimension, None);
        assert!(!backing.optional);
        let rendered = serde_json::to_string(&contract).unwrap();
        assert!(!rendered.contains("dimension"), "{rendered}");
        assert!(!rendered.contains("optional"), "{rendered}");
    }
}
