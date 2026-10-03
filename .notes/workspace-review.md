# Workspace review

> **Delete an item when you address it.** This file lists open findings only: no "done"
> markers, no history. When a group has no items left, delete its heading too.

Scope: production code of `common`, `quickbench`, `fits-well`, `imaginarium`, `scenarium`,
`lumos`, `lens` and `darkroom` (`palantir` is out of scope). Tests and the APIs they use were
not reviewed. Findings already listed in `lumos/.notes/*.md` are not repeated here.

Items are anchored to file paths and symbol names; line numbers, where given, go stale fast.
Groups are named after their shared root cause and ordered by severity, then benefit.
"Probe" means the claim was confirmed by running code against a scratch copy.

---

# High — wrong results, data loss, crashes on user data

# Medium — wrong in edge cases, duplicated truths, hot-path waste

# Low — duplication, dead surface, local simplifications
