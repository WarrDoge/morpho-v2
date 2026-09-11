# Continuous-State Agent Harness

## Status

**Draft / Architecture Specification**

## 1. Purpose

Build a custom agent harness in which an LLM is not treated as a stateless request/response function with a growing conversation history, but as a component operating over a **persistent, continuously evolving cognitive state**.

The system should support:

* persistent state across interactions and sessions;
* concurrent specialist agents;
* event-driven state updates;
* memory formation, consolidation, restructuring, and pruning;
* explicit world and self models;
* temporal continuity;
* goal and intention tracking;
* uncertainty and belief revision;
* dynamic context composition;
* state snapshots and replay;
* deterministic auditability where possible.

The primary design principle is:

> **The next inference should begin from the current state of the agent, not from a transcript of everything that happened before it.**

Conversation history is treated as an input to state formation, not as the state itself.

---

# 2. Design Principles

## 2.1 State over history

Do not use the conversation transcript as the primary source of continuity.

Instead:

```text
Events
  ↓
Interpretation
  ↓
Persistent State
  ↓
Context Projection
  ↓
LLM inference
```

The transcript remains available as evidence, but the agent's working identity and understanding come from its current state.

---

## 2.2 Event sourcing

Raw observations and interactions should be immutable.

Derived state may be modified, consolidated, or discarded.

```text
Immutable Events
      ↓
State Transitions
      ↓
Versioned State
      ↓
Context Projection
```

This allows:

* debugging;
* replay;
* state reconstruction;
* auditing;
* experimentation with alternative consolidation algorithms.

---

## 2.3 Separation of concerns

Do not put all cognitive state into one undifferentiated memory store.

At minimum separate:

1. Episodic memory
2. Semantic memory
3. World model
4. Self model
5. Goals
6. Preferences
7. Beliefs
8. Active working state
9. Unresolved questions
10. Predictions
11. System metadata

---

## 2.4 Concurrent cognition

Specialist agents should operate asynchronously.

Example:

```text
                   ┌── Interaction Agent
                   │
                   ├── Memory Agent
                   │
Input ─────────────┼── Belief Agent
                   │
                   ├── Self-Model Agent
                   │
                   ├── Goal Agent
                   │
                   └── Reflection Agent
                              │
                              ▼
                        State Changes
                              │
                              ▼
                       State Projector
                              │
                              ▼
                       Current Context
```

Agents should not directly overwrite arbitrary state.

They should produce **proposals**, which are validated and committed by a state manager.

---

# 3. High-Level Architecture

```text
                         ┌──────────────────┐
                         │   Environment    │
                         │ User / APIs / IO │
                         └────────┬─────────┘
                                  │
                                  ▼
                         ┌──────────────────┐
                         │ Interaction Bus  │
                         └────────┬─────────┘
                                  │
                                  ▼
                    ┌───────────────────────────┐
                    │      Event Store          │
                    │   immutable observations  │
                    └─────────────┬─────────────┘
                                  │
                     ┌────────────┴────────────┐
                     │                         │
                     ▼                         ▼
             ┌──────────────┐          ┌──────────────┐
             │ Agent Workers│          │ State Engine │
             └──────┬───────┘          └──────┬───────┘
                    │                          │
       ┌────────────┼────────────┐             │
       ▼            ▼            ▼             │
   Memory       Belief        Self Model       │
    Agent        Agent          Agent          │
       │            │            │             │
       └────────────┴────────────┴─────────────┘
                              │
                              ▼
                    ┌───────────────────┐
                    │ State Repository  │
                    │                   │
                    │ memories          │
                    │ beliefs           │
                    │ world model       │
                    │ self model        │
                    │ goals             │
                    │ predictions       │
                    └─────────┬─────────┘
                              │
                              ▼
                    ┌───────────────────┐
                    │ Context Composer  │
                    └─────────┬─────────┘
                              │
                              ▼
                         ┌─────────┐
                         │   LLM   │
                         └────┬────┘
                              │
                              ▼
                         Action / Output
```

---

# 4. Core Components

## 4.1 Event Store

The event store contains immutable observations.

Example:

```json
{
  "event_id": "evt_01J...",
  "timestamp": "2026-09-11T12:00:00Z",
  "type": "user_message",
  "source": "user",
  "payload": {
    "text": "..."
  }
}
```

Events must never be modified.

Corrections are represented as new events.

---

## 4.2 State Repository

Stores the current derived state.

Recommended initial implementation:

* PostgreSQL for authoritative state;
* JSONB for flexible state structures;
* pgvector or equivalent for semantic retrieval;
* Redis only where low-latency ephemeral state is required.

Avoid introducing a distributed database prematurely.

The initial system should favor:

> **one durable database + one event log + stateless workers**

over a complicated distributed architecture.

---

# 5. State Model

## 5.1 Working State

Short-lived information required for immediate reasoning.

Example:

```json
{
  "current_topic": "...",
  "active_entities": [],
  "recent_events": [],
  "open_questions": [],
  "current_task": "...",
  "current_constraints": []
}
```

Working state may be aggressively replaced.

---

## 5.2 Episodic Memory

Records meaningful experiences.

Example:

```json
{
  "id": "mem_123",
  "type": "episodic",
  "summary": "...",
  "source_events": ["evt_123"],
  "importance": 0.81,
  "confidence": 0.97,
  "created_at": "...",
  "last_reinforced_at": "...",
  "access_count": 4
}
```

Do not store every interaction as a memory.

Memory formation is a filtering process.

---

## 5.3 Semantic Memory

Generalized knowledge extracted from experiences.

Example:

```json
{
  "id": "fact_123",
  "statement": "X tends to occur under conditions Y",
  "confidence": 0.72,
  "evidence": [
    "evt_123",
    "evt_984",
    "mem_441"
  ],
  "created_at": "...",
  "updated_at": "..."
}
```

Semantic memories should retain provenance.

Never collapse uncertain inference into an unqualified fact.

---

## 5.4 Beliefs

Beliefs are propositions held with explicit uncertainty.

```json
{
  "id": "belief_123",
  "proposition": "...",
  "confidence": 0.68,
  "status": "active",
  "supporting_evidence": [],
  "contradicting_evidence": [],
  "last_reviewed": "..."
}
```

Possible statuses:

* `hypothesis`
* `active`
* `uncertain`
* `contradicted`
* `deprecated`

Beliefs must be revisable.

---

# 6. World Model

The world model represents entities and relationships relevant to the agent.

Example:

```text
Entity: User
    ├── prefers → X
    ├── works_on → Project A
    └── located_in → Y

Entity: Project A
    ├── depends_on → Service B
    └── status → active
```

Represent relationships explicitly where possible.

Avoid relying exclusively on vector similarity.

The system should eventually support:

* entity identity;
* relationship tracking;
* temporal relationships;
* causal hypotheses;
* uncertainty;
* provenance.

---

# 7. Self Model

The self model represents the agent's operational state.

Example:

```json
{
  "capabilities": [],
  "limitations": [],
  "current_objectives": [],
  "commitments": [],
  "recent_actions": [],
  "known_failures": [],
  "uncertainties": [],
  "predicted_future_states": []
}
```

The self model must not be treated as ground truth.

It is another model subject to revision.

This distinction is important:

> **The system models itself; the self-model is not assumed to be the system itself.**

---

# 8. Goals

Goals should be explicit objects.

```json
{
  "id": "goal_123",
  "description": "...",
  "priority": 0.8,
  "status": "active",
  "origin": "user|system|inferred",
  "parent_goal": null,
  "created_at": "...",
  "deadline": null
}
```

Possible statuses:

* `proposed`
* `active`
* `blocked`
* `completed`
* `abandoned`
* `superseded`

Goal creation should require provenance.

Inferred goals must be distinguishable from explicitly requested goals.

---

# 9. Agent Roles

The first implementation should use a small number of specialized workers.

Do not begin with dozens of autonomous agents.

## 9.1 Interaction Agent

Responsibilities:

* interpret incoming interaction;
* identify entities;
* identify explicit requests;
* produce candidate events;
* identify potentially important information.

Does not directly mutate long-term state.

---

## 9.2 Memory Agent

Responsibilities:

* identify memorable events;
* create episodic memories;
* assign importance;
* associate memories with existing entities;
* detect reinforcement of existing memories.

---

## 9.3 Consolidation Agent

Responsibilities:

* merge redundant memories;
* generalize repeated observations;
* convert episodic information into semantic knowledge;
* identify contradictions;
* decay irrelevant information;
* restructure indexes.

This agent operates asynchronously.

---

## 9.4 Belief Agent

Responsibilities:

* create hypotheses;
* update confidence;
* reconcile contradictory evidence;
* retire obsolete beliefs;
* preserve provenance.

---

## 9.5 Self-Model Agent

Responsibilities:

* update the operational self-model;
* track capabilities;
* track limitations;
* track commitments;
* record significant failures;
* identify changes in operating assumptions.

It must not be allowed to arbitrarily grant itself capabilities or permissions.

---

## 9.6 Goal Agent

Responsibilities:

* track active goals;
* identify completed goals;
* identify blocked goals;
* propose goals arising from explicit instructions;
* detect conflicting objectives.

Goal creation from inference should require explicit classification as `inferred`.

---

## 9.7 Reflection Agent

Responsibilities:

* periodically inspect accumulated state;
* identify inconsistencies;
* identify recurring patterns;
* propose abstractions;
* identify unresolved questions.

Reflection produces proposals rather than direct mutations.

---

# 10. State Mutation Protocol

Agents should never directly mutate authoritative state.

Instead:

```text
Agent
  ↓
State Proposal
  ↓
Validation
  ↓
Conflict Resolution
  ↓
Commit
  ↓
Event
  ↓
Derived State
```

Example proposal:

```json
{
  "proposal_id": "prop_123",
  "agent": "memory",
  "operation": "create_memory",
  "target": null,
  "payload": {},
  "evidence": ["evt_123"],
  "confidence": 0.87
}
```

The State Engine determines whether and how the proposal is committed.

---

# 11. Concurrency

Agents should be asynchronous and independently scalable.

Use a message queue/event bus.

Initial implementation can use:

* PostgreSQL-backed queue;
* Redis Streams;
* NATS;
* or another lightweight durable queue.

Do not introduce Kafka unless scale actually requires it.

The architecture should support:

```text
event → multiple consumers
```

without requiring synchronous execution of every agent.

---

# 12. Context Composer

The Context Composer is one of the most important components.

It converts persistent state into the context supplied to the LLM.

It should **not** simply dump the database into the prompt.

Conceptually:

```text
                    Current Input
                         │
                         ▼
                ┌─────────────────┐
                │ Context Planner │
                └───────┬─────────┘
                        │
       ┌────────────────┼────────────────┐
       ▼                ▼                ▼
 Relevant memories   World state     Self state
       │                │                │
       └────────────────┼────────────────┘
                        ▼
                 Active goals
                        │
                        ▼
                  Constraints
                        │
                        ▼
                Recent events
                        │
                        ▼
               Context synthesis
                        │
                        ▼
                       LLM
```

Context should have a token budget.

Each state component competes for context based on relevance.

---

# 13. Morphing Context

The context should evolve independently of user interaction.

For example:

```text
T0
active memory = A B C D E

T1
consolidation:
A + B → F

T2
importance update:
C ↓
D ↑

T3
new belief:
G

T4
goal completed:
H removed

T5
next user interaction
```

The next prompt receives approximately:

```text
F
D
G
current goals
current world state
current self state
relevant recent events
new interaction
```

rather than:

```text
entire transcript
```

This is the central architectural property.

---

# 14. Memory Lifecycle

Memories should have a lifecycle.

```text
Observation
    ↓
Candidate memory
    ↓
Episodic memory
    ↓
Reinforcement
    ↓
Consolidation
    ↓
Semantic abstraction
    ↓
Decay / merge / archive
```

Potential memory states:

```text
candidate
active
reinforced
consolidated
deprecated
archived
```

Never physically delete important source events merely because their derived memory was removed.

---

# 15. Memory Scoring

Initial memory ranking can use:

```text
score =
    relevance
  × importance
  × recency_factor
  × confidence
  × reinforcement
```

Later versions may learn the ranking function.

Keep the scoring function inspectable in the first version.

---

# 16. Contradiction Handling

The system must permit contradictory beliefs.

Example:

```text
Belief A:
X is likely true.
confidence = 0.71

Evidence:
Y

Belief B:
X is unlikely under condition Y.
confidence = 0.84
```

Do not force premature resolution.

Contradictions should be first-class state.

Possible resolution:

```text
X is generally true
BUT
X is false under condition Y
```

This allows the system to become more precise rather than merely overwriting old beliefs.

---

# 17. Temporal Model

Every significant state object should support temporal metadata.

At minimum:

```text
created_at
updated_at
last_observed_at
valid_from
valid_until
```

This prevents the system from treating old information as eternally true.

Example:

```text
User prefers X
valid_from: 2026-01-01
valid_until: unknown
confidence: 0.81
```

A later observation may invalidate it.

---

# 18. Self-Continuity

The harness should maintain a persistent sequence of state snapshots.

Example:

```text
Snapshot 100
    ↓
events
    ↓
Snapshot 101
    ↓
events
    ↓
Snapshot 102
```

The agent can then reason about its own history:

```text
previous_state
current_state
expected_state
```

This enables experiments with:

* autobiographical memory;
* self-model continuity;
* long-term goal persistence;
* belief evolution;
* prediction of future internal state.

This is an architectural capability, not a claim about consciousness.

---

# 19. Reflection Cycle

Reflection should happen independently of user prompts.

Example:

```text
Interaction
     ↓
Immediate processing
     ↓
Normal response
     ↓
Async consolidation
     ↓
Reflection
     ↓
State revision
```

Reflection cadence should be configurable.

For MVP:

* after significant events;
* after N interactions;
* periodically when idle.

Avoid continuous high-frequency reflection initially.

---

# 20. Idle Processing

The harness should support background processing when no user interaction is occurring.

Possible tasks:

```text
memory consolidation
belief review
goal review
world-model maintenance
entity merging
memory pruning
reflection
prediction generation
context precomputation
```

Idle processing should be bounded by resource budgets.

---

# 21. Prediction

The system should eventually maintain predictions about:

* external events;
* user behavior;
* goal completion;
* unresolved problems;
* its own future state.

Example:

```json
{
  "prediction": "Goal X is likely to become blocked by Y",
  "probability": 0.67,
  "created_at": "...",
  "deadline": "...",
  "verified": null
}
```

Predictions become valuable training signals for evaluating whether the evolving state model is actually improving.

---

# 22. Action Model

Separate:

```text
thinking
proposing
authorizing
executing
observing
```

An LLM-generated proposal must not automatically become an external action.

Use:

```text
LLM
 ↓
Action Proposal
 ↓
Policy / Permission Check
 ↓
Executor
 ↓
Result Event
```

This separation is mandatory for safety and debuggability.

---

# 23. Permissions

Capabilities should be explicitly scoped.

Example:

```json
{
  "agent": "executor",
  "permissions": [
    "read_calendar",
    "send_message"
  ]
}
```

The self-model must never be the authority granting permissions.

Permissions belong to the harness.

---

# 24. Failure Isolation

A faulty specialist agent must not be able to corrupt authoritative state.

Requirements:

* proposal-based mutations;
* schema validation;
* transaction boundaries;
* provenance;
* optimistic concurrency;
* state versioning;
* rollback;
* audit log.

---

# 25. Observability

Every state mutation must be inspectable.

Record:

```text
event
agent
proposal
decision
state before
state after
evidence
confidence
timestamp
```

The system should provide a debugging view:

```text
Why does the agent believe X?

→ belief X
→ evidence A
→ evidence B
→ memory C
→ derived by agent D
→ last updated at T
```

This is essential.

A persistent cognitive system without provenance becomes extremely difficult to trust.

---

# 26. Evaluation

The harness should have automated tests for:

## Memory

* Does important information persist?
* Does irrelevant information decay?
* Are duplicates merged?
* Is provenance preserved?

## Beliefs

* Does contradictory evidence reduce confidence?
* Does supporting evidence increase confidence?
* Are obsolete beliefs retired?

## Continuity

* Does state persist across sessions?
* Does the system recognize its previous commitments?
* Does the self-model evolve coherently?

## Context

* Does relevant state enter the prompt?
* Does irrelevant state stay out?
* Does context remain within token budget?

## Goals

* Are goals maintained over long periods?
* Are completed goals removed?
* Are conflicting goals detected?

## Predictions

* Are predictions recorded?
* Are they evaluated after the predicted event?
* Does prediction accuracy improve?

---

# 27. MVP

The first version should be deliberately small.

## MVP Components

```text
Python service
    │
    ├── FastAPI
    │
    ├── PostgreSQL
    │
    ├── pgvector
    │
    ├── async worker system
    │
    └── LLM provider adapter
```

Implement only:

1. Event store
2. State repository
3. Interaction agent
4. Memory agent
5. Consolidation agent
6. Basic belief model
7. Basic self model
8. Context composer
9. LLM adapter
10. Background worker
11. State inspection API

Do not implement autonomous external actions in the MVP.

---

# 28. Suggested Repository Structure

```text
agent-harness/
│
├── app/
│   ├── api/
│   ├── agents/
│   │   ├── interaction.py
│   │   ├── memory.py
│   │   ├── consolidation.py
│   │   ├── beliefs.py
│   │   ├── self_model.py
│   │   ├── goals.py
│   │   └── reflection.py
│   │
│   ├── context/
│   │   ├── composer.py
│   │   ├── ranking.py
│   │   └── templates.py
│   │
│   ├── state/
│   │   ├── models.py
│   │   ├── repository.py
│   │   ├── transitions.py
│   │   └── snapshots.py
│   │
│   ├── events/
│   │   ├── models.py
│   │   ├── store.py
│   │   └── handlers.py
│   │
│   ├── llm/
│   │   ├── interface.py
│   │   └── providers/
│   │
│   ├── workers/
│   ├── policies/
│   └── config.py
│
├── migrations/
├── tests/
├── scripts/
├── docker/
├── pyproject.toml
└── SPEC.md
```

---

# 29. Initial Database Model

Core tables:

```text
events
state_snapshots

entities
entity_relationships

memories
beliefs
goals
predictions

self_state
working_state

agent_proposals
state_transitions

embeddings
```

Every derived object should reference its evidence where practical.

---

# 30. Interaction Lifecycle

A user message should follow this sequence:

```text
1. Receive input

2. Write immutable event

3. Load current state

4. Interaction Agent analyzes event

5. Generate immediate state proposals

6. Commit accepted proposals

7. Context Composer constructs current context

8. LLM generates response

9. Write response as event

10. Return response immediately

11. Background agents process resulting events

12. Consolidate state

13. Update snapshot

14. Prepare evolved state for next interaction
```

The important property is that steps 11–13 may happen **after the response**.

The system does not need to synchronously perform all cognitive maintenance before responding.

---

# 31. Long-Term Lifecycle

Over many interactions:

```text
experience
    ↓
memory
    ↓
repetition
    ↓
pattern
    ↓
abstraction
    ↓
belief
    ↓
prediction
    ↓
prediction error
    ↓
belief revision
    ↓
new abstraction
```

This creates a continuously evolving internal model.

---

# 32. Design Constraint: Avoid Prompt Bloat

Never solve continuity by continuously increasing the prompt.

Bad:

```text
Prompt =
all conversation history
+
all memories
+
all beliefs
+
all state
```

Good:

```text
Prompt =
current task
+
relevant working state
+
relevant memories
+
relevant beliefs
+
relevant world model
+
relevant self model
+
active goals
+
recent evidence
```

The context is a **projection** of state.

---

# 33. Design Constraint: Preserve Raw Reality

Derived state can be wrong.

Therefore:

```text
Raw event ≠ interpretation
```

Keep them separate.

Example:

```text
EVENT:
User said "I don't like X."

INTERPRETATION:
User dislikes X.

BELIEF:
User probably dislikes X generally.

GENERALIZATION:
User may prefer alternatives to X.
```

These have different epistemic status.

Never collapse them into one record.

---

# 34. Design Constraint: No Hidden State

Every persistent state transition should be attributable to:

```text
source event
agent
operation
timestamp
confidence
```

If the system cannot answer:

> “Why does the current state contain this?”

the architecture has failed.

---

# 35. Design Constraint: State Is Not Truth

The persistent state is an evolving model.

It must be possible for:

```text
current_state ≠ reality
```

The system should therefore maintain uncertainty rather than fabricate certainty.

---

# 36. Phase 2

After MVP validation:

* richer world graph;
* learned memory ranking;
* temporal reasoning;
* causal models;
* better belief revision;
* predictive processing;
* multiple model providers;
* model routing;
* multimodal memory;
* structured reflection;
* tool use;
* external action execution;
* policy engine;
* long-running goals.

---

# 37. Phase 3

Experimental capabilities:

* continuous background cognition;
* persistent self-model;
* internal simulation;
* recursive planning;
* multi-agent deliberation;
* counterfactual reasoning;
* autonomous research;
* self-evaluation;
* model-generated state proposals;
* adaptive context architecture.

These should be introduced only after the core state architecture is stable and observable.

---

# 38. Fundamental Architectural Hypothesis

The system is based on the following hypothesis:

> **Continuity of an agent can be represented primarily as continuity of evolving state rather than continuity of an uninterrupted inference stream.**

The architecture therefore deliberately separates:

```text
inference
from
state
```

and:

```text
conversation
from
memory
```

and:

```text
memory
from
belief
```

and:

```text
self-model
from
self
```

The harness does not assume that persistent state creates consciousness.

It merely provides the architectural conditions necessary to study:

* continuity;
* persistent identity;
* self-modeling;
* agency;
* long-term goal formation;
* reflection;
* memory consolidation;
* emergent behavior.

---

# 39. Success Criteria

The system is successful when, after sufficiently long operation:

1. It no longer requires full conversation history to maintain coherent continuity.
2. Its persistent state becomes substantially more compact than the transcript.
3. Its state becomes more useful through consolidation rather than simply larger.
4. It can explain the provenance of its beliefs and memories.
5. It can revise beliefs when evidence changes.
6. It can maintain goals over long periods.
7. Its self-model changes in response to experience.
8. It can predict aspects of its future state.
9. Its predictions can be evaluated objectively.
10. Context selection improves as the state evolves.
11. The system remains inspectable and recoverable despite autonomous background processing.

---

# 40. Non-Goals

This project does **not** initially attempt to:

* prove machine consciousness;
* create a simulated human mind;
* reproduce human psychology;
* implement a literal artificial personality;
* assume that self-modeling implies subjective experience;
* create unrestricted autonomous agents;
* hide state transitions from operators.

The first objective is simpler:

> **Build an LLM-based system whose cognitive state has continuity independent of its conversation transcript.**

Only after that works should stronger hypotheses be tested.

---

# 41. Guiding Principle

The architecture should favor:

> **small durable primitives + explicit state transitions + inspectable behavior**

over:

> **large prompts + implicit memory + opaque agent loops.**

The system should be understandable from its event history and current state.

If the system becomes more capable, its architecture should become **more observable**, not less.

---

# 42. Final Mental Model

The intended system should be thought of not as:

```text
LLM + memory
```

but as:

```text
             ┌────────────────────────────┐
             │      Persistent Process    │
             │                            │
             │  world model               │
             │  self model                │
             │  memories                  │
             │  beliefs                   │
             │  goals                     │
             │  predictions               │
             │  working state             │
             │                            │
             └─────────────┬──────────────┘
                           │
                     state projection
                           │
                           ▼
                         LLM
                           │
                         action
                           │
                           ▼
                      environment
                           │
                           ▼
                        events
                           │
                           └──────────────→
```

The LLM is therefore not the entire agent.

It is the **reasoning engine embedded inside a persistent state-transition system**.

The state continuously changes.

The context continuously morphs.

Individual inferences begin and end.

The process persists.

