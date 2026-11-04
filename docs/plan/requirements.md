# What David asked for

Date: 2026-11-04. Written by the architect from David's own words. The architect is the helper that keeps this whole piece of work consistent; David is the owner.

## The goal

A terminal program with a text user interface, written in Rust with the Ratatui library, for network labs and observability workshops. Its working name is nobs, the name of this repository.

## What it replaces and takes in

- It replaces labby, David's Python command line tool for GNS3 labs (`reference/labby`).
- It takes in labcli, his Python tool for containerlab and docker compose labs (`reference/labcli`).
- It takes in the lab tooling of the workshops repository (`reference/workshops`), which already holds a small Python command line tool also called nobs.
- It takes in the lab of the book "Modern Network Observability" (`reference/network-observability-lab`), which David wants to bring back to life for the book's second edition, replacing its netobs tool.

## Sonda

Added on 2026-11-04 from David's words: "on this new TUI application, we definitely need to have an interface for Sonda". Sonda is David's synthetic telemetry generator (`reference/sonda`, public, Rust). It has an HTTP API and a way to load files (scenarios), and the workshops' Python nobs already connects to it. The terminal program must be a way to interact with Sonda: load scenario files and work with running scenarios through its API. Sonda's own documentation is the source for what the API offers.

## Where it is used

In all of David's repositories, labs and workshops, on his own computer and in cloud sessions.

## Decided so far

- The repository is private while the program is planned and built. David may make it public later. So the workbench's own files (the `.claude` folder) are never committed here; they live only on David's computer.
- The four source repositories are public. Copies of them sit under `reference/`, which git ignores. They are read, never changed.
- The sessions that plan and build nobs run on David's computer. The architect answers their questions and brings David only the decisions that are his.

## Not decided yet

- Which lab systems the first version must support (GNS3, containerlab, docker compose, or all).
- Whether the small Python nobs in the workshops repository is replaced by this program, or stays next to it.
- How the program is installed: one file to download, a package, or both.
- What stays out of the first version.
