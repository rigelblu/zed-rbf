---
title: Changelog
---

# 🔵⋯ Convention
Document only user-facing changes for this product. For technical products (APIs, platforms, dotfiles), developers are the end-users. Skip churn and internal-only work.

**Filter out:** drop entries that don't carry user value — internal refactors, no-impact bumps, churn. If it wouldn't match `vcs-change-summary product` framing, it doesn't belong here.

**Entry format:** use skill `vcs-change-summary product`. Prefix each entry with today's date:
`- YYYY-MM-DD - $change-what ($change-action $scope) | $user-need [@$product]`

**Sections:** `Added` · `Improved` · `Fixed` · `Removed` · `🚨 Breaking Changes` · `🔒 Security Fixes`

# 🔵⋯ Refuse common rationalizations
- "Just log everything that changed." → User-facing only — internal churn rots the changelog
- "The commit message is enough." → Translate to user-need framing via `vcs-change-summary product`; commits often say *what* not *why for user*

---

# Example
```md
# 🔵⋯ [Unreleased]
## 🟠⋯ Added
- 2026-01-01 - feat (user need) | provide product category for cicd in each repo [@dotfiles]

## 🟠⋯ Fixed
- 2026-01-15 - fix (ux) | python alias to latest version [@zsh]

# 🔵⋯ v0.2.0
## 🟠⋯ Improved
- 2025-12-30 - refactor | upgrade to svelte v4 [@rb-site]
```

# Template
```md
───
# 🔵⋯ [Unreleased]
## 🟠⋯ 🚨 Breaking Changes
- YYYY-MM-DD - <change summary>

## 🟠⋯🔒 Security Fixes
- YYYY-MM-DD - <change summary>

## 🟠⋯ Added
- YYYY-MM-DD - <change summary>

## 🟠⋯ Improved
- YYYY-MM-DD - <change summary>

## 🟠⋯ Fixed
- YYYY-MM-DD - <change summary>

## 🟠⋯ Removed
- YYYY-MM-DD - <change summary>

───
# 🔵⋯ v#.#.# (YYYY-MM-DD)
## 🟠⋯ 🚨 Breaking Changes
- YYYY-MM-DD - <change summary>

## 🟠⋯🔒 Security Fixes
- YYYY-MM-DD - <change summary>

## 🟠⋯ Added
- YYYY-MM-DD - <change summary>

## 🟠⋯ Improved
- YYYY-MM-DD - <change summary>

## 🟠⋯ Fixed
- YYYY-MM-DD - <change summary>

## 🟠⋯ Removed
- YYYY-MM-DD - <change summary>
```
