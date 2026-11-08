---
name: release-notes
description: Writes release notes from merged pull requests
when_to_use: When the user asks for release notes or a changelog
argument-hint: "[from-tag] [to-tag]"
arguments: [from, to]
disable-model-invocation: true
user-invocable: false
allowed-tools: Read, Grep, Bash(git log *)
disallowed-tools: Edit
model: sonnet
effort: high
context: fork
agent: general-purpose
paths: "docs/**/*.md, CHANGELOG.md"
shell: bash
---
Write notes for the changes between $from and $to.
