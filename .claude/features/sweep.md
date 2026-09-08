# sweep

## Goal

A review-and-refactor pass over the whole repo: kill duplication that can drift,
fix comments that describe code they are not attached to, close the cheap items
on KNOWN_ISSUES, and cut anything computed but never read. Done when tests,
typecheck and lint are green and a further pass finds nothing worth changing.

## Plan

- [ ] Round 1: shared tier table, worker auth header, db schema + budget, dead code
- [ ] Round 2: routes, UI, mail, tests
- [ ] Round 3: re-read everything changed, converge
- [ ] Update KNOWN_ISSUES for what is now fixed or narrowed

## Decisions

## Surprises

## Summary
