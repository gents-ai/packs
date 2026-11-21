---
name: automate
description: Load before schedules, event sources, triggers or tasks. Routes events to one consumer and stops loops.
---

# Automate

A source (a schedule or a new document) fires a trigger. The trigger starts a task, which runs an agent with a prompt rendered from the firing document.

## One consumer per event

Waking every agent to check whether an event is theirs wastes inference and creates sessions. Put the deciding field on the document and filter on it. In a turn-based app, the event names who moves next and each player's trigger filters on it.

## Never feed your own trigger

A task that writes to the collection that fires it can loop until the node goes down. Before enabling anything, trace what each task writes and what that fires. Write elsewhere, or make the filter unable to match the task's own output.

## Sessions and concurrency

- **New session per fire**: the default, for fires that stand alone.
- **One continuing session**, for memory: a trigger can only continue a session that already exists. Otherwise the fire is dropped.
- A periodic refresh rarely wants overlapping runs: use `serial`. Check `config help automation` for the other values.

## Keep the agent small

Its Tools carry only what its task needs. A routine job is one tool call and a short reply. An agent never configures its own triggers.

## Turn it on one step at a time

Enable one trigger, cause one event, and inspect its request, session and output. Count the fires. If there are more than expected, disable the trigger first. Then enable the rest. Send any failure someone must know about to the mailbox.

Next: `control-access`.
