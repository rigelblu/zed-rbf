---
title: Use skills when starting work with the product and its projects
---

# 🔵⋯ Common Project Rules
Session memory, the product plan, and pickup notes live on the external
rb-drive — not in this repo. Don't start cold; resume.

## 🟠⋯ Session lifecycle
- **Start** — `session-block-pick-up minimal generalist`: loads prior memory, the ship plan, and what to pick up
- **Before designing or building anything** — `conduct-product-dimensions`
- **End** — `session-block-wrap-up`: writes memory and pickup notes back

### 🟣⋯ The exemption has to speak
Both of the above may be skipped for a genuine throwaway one-off. **Say so before your first edit, or you have not taken the exemption — you have skipped the rule.**
> Treating this as a one-off, so no dimensions pass: `<why>`.

The trigger is **your first edit to a source file**, not your read of whether the request is "a feature". That distinction is the whole point.
e.g. *"lower the contrast of the background color of workspaces"* was classified as a one-off and built, tested and handed back before `conduct-product-dimensions` was invoked. It was a feature: it earned a ship-plan entry, a design finding that redirected the next change, and it exposed a latent renderer-drift defect in the code it touched. Nothing about the opening request looked like that.

So the rule cannot key on the request. An agent that has already decided "this is small" will not recognise itself in *"when starting a new feature."* It will recognise itself in *"I am about to edit a file and I have not said which mode I am in."* One visible sentence costs nothing when the call is right, and when it is wrong it surfaces in the second message instead of the eighth.

**Refuse this rationalization:** *"I'll do the pass afterwards if it turns out to matter."* → It did turn out to matter here, and the pass then ran on an existing implementation, which ratifies rather than verifies. A gate downstream of the thing it gates is not a gate.

## 🟠⋯ Where it lives (reference — the skills above resolve these for you)
`prj-use` fills `<PROJECT_NAME>`. If a path here ever disagrees with reality, trust the skill.

### 🟣⋯ Session block — `rb-drive/agents/<PROJECT_NAME>/`
- **Core Memories:** `agents-core-memories.md`
- **Turning Memories:** `agents-turning-memories.md`
- **Find My Bearings:** `agents-find-my-bearings.md`
- **Agent & Human Shared Understanding:** `agents-shared-understanding.md`
- **What to pick up next session:** `agents-pickup.md`

### 🟣⋯ Product — `rb-drive/projects/<PROJECT_NAME>/06_plan-execute/`
- **Product/Feature Ship Plan:** `product-ship-plan.md`
- **Released Features:** `product-ship-released.md`

### 🟣⋯ Project — `rb-drive/projects/<PROJECT_NAME>/06_plan-execute/`
- **Project Workblock Plan:** `project-wb-plan.md`

---

# 🔵⋯ Custom Project Rules
(empty)
