# Incremental parsing, exactly

The incremental parser's contract: after any edit, the tree it keeps is
the tree a batch parse of the new text builds, and the repairs it reports
are the repairs batch makes. "The tree" is `semantic_eq`: structure, trivia
placement, and the two facts each node carries about how it was parsed
(`GreenNode::lr_ends`, `GreenNode::recovery`), since the next incremental
parse reuses nodes by them. The contract holds for broken text as for
clean text: recovery runs wherever the text is broken, and batch's
recovery decisions are reproduced, not approximated.

`crates/qana-lang/tests/edit_scripts.rs` holds the parser to it after
every step of random edit scripts over the C demo (most steps leave it
broken). The wide sweep there (512 scripts, 81,920 edits, release build)
is how the rules below were found; each rule has a test made from the
smallest document that broke it.

## Reuse is state matching

A node is reused only where the parse stands in the LR state the node's
own parse began in (`LrEnds::begin`). The automaton is deterministic and
the node's tokens are as they were, so a batch parse from that state
rebuilds the node exactly, except at its right edge: the node's last
reductions (its own, and those of every node ending where it ends) were
decided on the terminal after it. Each node records the state its last
reduction was decided in (`end`) and the production it reduced by
(`prod`); at the splice, the parser checks that each of those states calls
for that reduction on the terminal that now follows (`reduces_alike`). The
check is on the action, not the terminal: inserting a statement after an
item does not cost the item its reuse.

This replaces Wagner's sentential-form rule (splice wherever the
automaton has a GOTO for the node), its right breakdown (undo a splice
when the parse errors after it), the precedence "fragile" productions,
and the list-alignment checks. Those were exact only for text that parses
clean; under recovery the stack batch has at an error differs from the one
they reconstruct, and 91 of 10,240 random edits diverged with them. State
matching needs no undoing: nothing reused is ever wrong.

## What a node's ends cannot vouch for

`lr_ends` is `None`, and the node is never reused whole, when its last
reduction was bound up with a recovery decision:

* recovery made it: a reduction on a token recovery inserted, or the
  error-time closure over default reductions (which recovery skips once
  the repair budget is spent);
* a recovery came between its last symbol and its reduction.

An ε-node (or a node of them) takes no symbol, and so has two places: it
begins where it is reduced and ends just after the symbol before it. A
recovery that deletes terminals and leaves an ε-reduction to be decided on
the one it exposed falls between the two. A node the ε-node ends stops
before that recovery: it neither counts the deletions among its own
repairs nor vouches for its ends (the deleted terminals are material in
front of the next symbol, outside it). A node the ε-node begins starts
after it, as any node starts after the material in front of it. The
ε-node itself vouches for its ends: the state and the terminal decide an
ε-reduction, whatever came before them, and an incremental parse reusing
the node that holds it, parsed where nothing was skipped, decides it the
same way.

A node without a terminal (an ε-node, an empty list) is never reused at
all. It saves nothing, and nothing in the tree says what it was reduced
on: the tree places it in front of the material pending when it was
reduced, and in front of error material that may have been a token
recovery then skipped, or the token after the material.

## Lists

Lists are balanced trees of RUN chunks (envelope L4). Chunks are cut only
between repetition units, so a chunk of whole units entered in the list's
state `c` leaves the parse in `c`, as parsing it would: it begins in `c`
and can continue any open list of its kind standing in `c`. Its end is the
state its last unit's symbols lead to from `c`, computed when the list is
finalized. The chunk holding the seed began with no list open, so no open
list can continue with it: it has no `lr_ends`, and neither does a chunk
cut inside an oversized unit or holding an unsettled piece. A whole list
is reused through its GOTO from the state it began in.

## Nodes carrying an error

Recovery's decisions depend on more than the top state:

1. the whole state stack (`simulate`, `eof_progress`, and
   `la_wanted_below`, which scans up to 128 states down);
2. which stack entries are lists, and whether those have pieces (list
   boundaries skip; the set-aside probe stops at a non-empty list);
3. the next `FAR` terminals (`FAR` = 32), which rank the repairs that
   tie on the K-terminal window (see below);
4. the repair-density state (where the last recovery was entered, and its
   cluster size);
5. the repair budget.

So a node holding an error of its own carries a record
(`GreenNode::recovery`): the stack beneath it, the kinds of the entries
there, its repairs and the terminals its recoveries were entered at
(relative to its first terminal), the cluster size its last recovery left,
and the density and budget it began with, kept only as far as recovery
inside it can tell them apart:

* density: whether its first recovery lands within
  `REPAIR_CLUSTER_WINDOW` of the last one before it, and if so that
  cluster's size;
* budget: nothing while the budget cannot be reached inside it, the exact
  count before it when it can.

It is reused where all of that is as recorded and the `FAR` terminals
after it are unchanged (salvage keeps that margin). Splicing it appends its
repairs, shifted to where it now stands, and sets the density state to its
recorded end. Because a record keeps only what recovery can tell apart, a
node reused under a configuration that differs only where recovery cannot
see, and every node beneath it, keeps a record equal to batch's.

A node gets no record when it began where a recovery had just been entered
(its first symbol was decided by that recovery), when its own first
recovery was entered at its first terminal (that recovery's material can
precede its first symbol), or when it made no repair of its own.

## Ranking repairs

A repair is up to three steps at the error (`search_repairs`), each one of:

* inserting a token before the lookahead;
* deleting the lookahead;
* setting the construct in progress aside as error material, its top
  entries popped until a state beneath acts on the lookahead (only as a
  first step, and only for a lookahead a construct beneath wants; see
  below).

A repair is complete when the parse then validates at least one of the
next K terminals; one that does not is extended by a step (repairs leaving
the same stack at the same point are extended once). Of the complete
repairs, the winner is the one under which the parse gets furthest
through the next `FAR` terminals, a deleted terminal counting as passed,
and then:

1. the cheaper (fewer steps);
2. the one inserting fewer brackets;
3. the one giving up fewer of the user's terminals (deleted, or in the
   entries set aside);
4. the one leaving the stack shallower (a fabricated body or parameter
   list absorbs unboundedly);
5. the smaller sequence, insertions before deletions and tokens in order,
   so the choice never depends on the order repairs were found.

A complete repair reaching the end of the `FAR` terminals cannot be beaten
by a costlier one, and ends the search. Among insertions of one length the
order is the window's score and then the far reach, as it was when only
insertions were searched; what the far reading adds is choosing between
lengths and kinds. In `[a, x[b, c], d]` (an `x` typed into an array of
pairs) the cheapest repair, inserting `]` before the index's `,`,
validates `, c ]` and then closes the array at the real `]`, and the
rest of a 20,000-element table broke into 200 repairs. Replacing the `,`
costs a step more and validates everything after it.

An inserted bracket is a claim about structure past the window: every
real bracket after it pairs differently. After `for p in v {x` and before
`match p { … }`, inserting `;` and inserting `}` read equally far, and
`}` leaves the stack shallower; but it closes the loop's body, the loop's
own `}` then closes the function, and the rest of the file came apart.
So fewer inserted brackets comes before the depth.

A terminal shifted in a permissive state, one that shifts or reduces on
every terminal the grammar uses except the closing brackets (a token
tree's), validates only what its brackets do. Inside a nest of token
trees one of whose openers the repair inserted it validates nothing, and
the simulation feeds it on without counting it: otherwise inserting `! (`
after a stray name would make the rest of the file valid inside a macro
invocation, the real brackets in it nested trees of their own. Inside
trees the user opened it is read as written, so a macro call in the way
(`debug_assert!(…)`, `#[serde(rename = "…")]`) neither ends a reading
nor leaves it short of the end of the window, where the search stops.
`LrTables::permissive` is computed with the tables, from the grammar's
bracket pairs.

A lookahead some construct beneath the top still wants (a closer, a
separator) is never deleted: deleting the `}` of a struct hands what
follows to the member in progress, a catastrophe the window cannot see.
Setting the construct in progress aside is a step of its own, ranked with
the rest: after `}x` before an impl's closing `}`, setting `x` aside
reads on as written, where inserting `! {` makes the real `}` the closer
of a fabricated macro call and leaves the impl open. Where nothing can be
set aside, a repair keeping the lookahead must get as far as deleting it
would, and otherwise the lookahead is deleted. A bracket opened on the
stack and not yet closed holds everything up to its closer: nothing
beneath it wants a token that is not that closer (`LrTables::opens` marks
the states entered by an opener). In `x[b, c]` inside an array, the `,`
belongs to the index, whatever the array would take.

## Material in front of a node

What a parse drains in front of a node's first symbol was decided before
the node began. When it holds error material (a skipped token, an entry
set aside), it goes back to the input and is decided again where the new
parse stands (`strip_lead`); the node follows. If an ε-reduction comes
before the node's first symbol down its left spine (an ε-node, or the ε
seed of a list), the node cannot be reused whole: that reduction may have
been made on a token the material held. Those nodes go back as their
children, down to the first node on the spine with no ε-reduction in front
of its first symbol.

## Performance

`splice_edit_cost.rs` `error_dense_document_keystroke` (one `%%` error in
every function), partial-token keystroke, release:

| functions (errors) | before (sentential form, unsound) | state matching |
|---|---|---|
| 1,000 | 12.5 ms | 4.2 ms |
| 5,000 | 82 ms | 24 ms |
| 27,000 | 458 ms | 127 ms |

The cost still grows with the document: past `MAX_REPAIRS` repairs, the
rest of the input is set aside as error material, and that region is
re-read on every keystroke.
