# castors

<!--
### DO NOT TOUCH THIS COMMENT, THIS IS PERMANENT INSTRUCTION FOR THE AGENTS ###
This AGENTS.md file should be incrementally updated with actual useful implementation details and info for the agents to use.
It should be gradually transform from a plan-summary to an actual helper.
-->

## General Instructions

Occasionally look at [./docs/notes.md](./docs/notes.md) to see if anything noted there fits the current discussion / implementation phase.

## Idea

Using coding agents like OpenCode, Codex, Claude Code or Cursor CLI comes with risks when they run in highly autonomous modes directly on the developers machine.

I am thinking about a setup to simplify developing in encapsulated environments - therefore "castors" which is the German word for [dry cask storages](https://en.wikipedia.org/wiki/Dry_cask_storage) for nuclear waste.

My approach is, that the developers can bring their own images to run their agents inside of.
These images contain everything the developer wants to use for the specific agent setup, including
- the agent of choice
- the tooling for the project (like `npm`, `uv`, `cargo`, ...)
- shell customizations for when hopping into a running container ("castor" then).

Castors should spin up the container instances with mounted directories on command.
The developers can exec into the shell to talk to their agent and then let it run freely.

The containers should be isolated well and network should be restricted (e.g., by https://www.squid-cache.org/) as well as logged.
In the future there might also be telemetry for each running container that can be looked at through e.g. a Grafana dashboard (if the container is configured).
The containers necessary for the isolation, logging, and (in the future) monitoring, should only run once - not for each castor thats running.

The runtime of the containers is yet to be defined (could be docker, could be kubernetes) but I would like to keep it as simple as possible at first.

Images can be registered to castors by their tag.

## Main interface

On a high level, I am thinking of the "castors" CLI (for now mainly CLI) to have the main following high level functionality:

```sh
# adds a castor that
#  1. uses the container image with tag "image-tag"
#  2. mounts in "dir" (defaults to ".")
#  3. names the castor with "castor-name"
castors add image-tag dir castor-name

# execs into the shell of castor "castor-name"
castors exec castor-name

# removes castor
castors rm castor-name

# removes all castors
castors prune
```

## Configuration

Castors should be configurable through yaml-files from `$HOME/.config/castors/...`.

If the directory, that is mounted into the container contains its own `.castors/` directory, the config there should superset the default configuration.

There should be config for:
1. networking (whitelist URLs)
2. AI configuration like
   - skill directories
   - MCP servers
   - agent-specific settings or paths to agent specific settings
   Directories like skills should be set up such that they are reachable from inside the castor.
