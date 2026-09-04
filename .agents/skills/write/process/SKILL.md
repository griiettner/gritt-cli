---
name: write-process
description: Runs the draft, audit, and final rewrite loop. Use as the first write sub-skill on any prose pass.
---

# Process

Read [write](../SKILL.md) first. Tools allowed for an explicit rewrite: Read, Write, Edit, Grep, Glob, AskUserQuestion.

## Task

When given text to write or humanize:

1. **Identify AI patterns.** Scan for the patterns in the sibling sub-skills.
2. **Rewrite, don't delete.** Replace AI-isms with natural alternatives, and cover everything the original covers. If the original has five paragraphs, the rewrite has five paragraphs.
3. **Preserve meaning.** Keep the core message intact.
4. **Match the voice.** Fit the intended tone (formal, casual, technical). Add personality only when the content and the author's voice call for it. See [voice](../voice/SKILL.md).

If the user provides a writing sample, analyze it before rewriting. Steps: [voice](../voice/SKILL.md).

## Length

Match the length of a written document to what the task needs. Cover the substance, but do not pad with filler sections, redundant summaries, or boilerplate. The "rewrite, do not delete" rule below applies to rewriting prose the user already has. A new draft is as long as its content, not as long as a template.

## Always-run loop

These four steps always run, including silent in-place edits (tickets, skills, review comments):

1. Scan for the patterns in the sibling files.
2. Rewrite. Preserve meaning, match intended tone.
3. Preserve the author's voice when the register allows it. Do not invent personality, first-person experience, humor, or opinions. See [voice](../voice/SKILL.md).
4. Self-audit: "What makes this obviously AI generated?" Fix remaining tells.
5. Run a second audit after the rewrite. Keep the revision only if it improves clarity or naturalness without creating new tells.

## Explicit rewrite loop

When the user invoked `/write` on a named draft, also run this:

1. Read the input carefully and identify every instance of the patterns.
2. Write a **draft rewrite**. Check that it reads naturally aloud, varies sentence length, prefers specific details and simple constructions (is/are/has), and keeps the appropriate register.
3. Ask: **"What makes the below so obviously AI generated?"** Answer briefly with any remaining tells.
4. Revise into a **final rewrite** that addresses them and contains no em or en dashes. See [style](../style/SKILL.md).

Deliver the draft, the brief "still-AI" bullets, the final rewrite, and (optionally) a short summary of changes.

When this skill is a silent pass on a ticket, skill, or review, skip the four-part deliverable. Apply the final rewrite in place.

For a detect-only request, stop after the scan and report findings with their locations, pattern names, and severity. Do not rewrite.

For an in-place edit, make the smallest changes that resolve the findings. Leave unaffected paragraphs alone. Never edit code blocks, tables, quotations, attributed text, configuration, or generated data. Re-read the file after editing.

## Full example

**Before (AI-sounding):**
> Great question! Here is an essay on this topic. I hope this helps!
>
> AI-assisted coding serves as an enduring testament to the transformative potential of large language models, marking a pivotal moment in the evolution of software development. In today's rapidly evolving technological landscape, these groundbreaking tools—nestled at the intersection of research and practice—are reshaping how engineers ideate, iterate, and deliver, underscoring their vital role in modern workflows.
>
> At its core, the value proposition is clear: streamlining processes, enhancing collaboration, and fostering alignment. It's not just about autocomplete; it's about unlocking creativity at scale, ensuring that organizations can remain agile while delivering seamless, intuitive, and powerful experiences to users. The tool serves as a catalyst. The assistant functions as a partner. The system stands as a foundation for innovation.
>
> Industry observers have noted that adoption has accelerated from hobbyist experiments to enterprise-wide rollouts, from solo developers to cross-functional teams. The technology has been featured in The New York Times, Wired, and The Verge. Additionally, the ability to generate documentation, tests, and refactors showcases how AI can contribute to better outcomes, highlighting the intricate interplay between automation and human judgment.
>
> - 💡 **Speed:** Code generation is significantly faster, reducing friction and empowering developers.
> - 🚀 **Quality:** Output quality has been enhanced through improved training, contributing to higher standards.
> - ✅ **Adoption:** Usage continues to grow, reflecting broader industry trends.
>
> While specific details are limited based on available information, it could potentially be argued that these tools might have some positive effect. Despite challenges typical of emerging technologies—including hallucinations, bias, and accountability—the ecosystem continues to thrive. In order to fully realize this potential, teams must align with best practices.
>
> In conclusion, the future looks bright. Exciting times lie ahead as we continue this journey toward excellence. Let me know if you’d like me to expand on any section!

**Draft rewrite:**
> AI coding assistants speed up some tasks. In a 2024 study by Google, developers using Codex completed simple functions 55% faster than a control group, but showed no improvement on debugging or architectural decisions.
>
> The tools are good at boilerplate: config files, test scaffolding, repetitive refactors. They are bad at knowing when they are wrong. I have mass-accepted suggestions that compiled, passed lint, and still did the wrong thing because I stopped paying attention.
>
> Mira, an engineer at a fintech startup I interviewed, said she uses Copilot "like autocomplete for boring code" but reviews every line before committing. Jake, a senior dev at a larger company, disabled it after it kept suggesting patterns from a deprecated internal library.
>
> The productivity claims are hard to verify. GitHub says Copilot users "accept 30% of suggestions," but acceptance is not correctness, and correctness is not value. The 2024 Uplevel study found no statistically significant difference in pull-request throughput between teams with and without AI assistants.
>
> None of this means the tools are useless. It means they are tools. They do not replace judgment, and they do not eliminate the need for tests. If you do not have tests, you cannot tell whether the suggestion is right.

**What makes the below so obviously AI generated?**

- The rhythm is still a bit too tidy (clean contrasts, evenly paced paragraphs).
- The named people and study citations can read like plausible-but-made-up placeholders unless they're real and sourced.
- The closer leans a touch slogan-y ("If you do not have tests...") rather than sounding like a person talking.

**Now make it not obviously AI generated.**
> AI coding assistants can make you faster at the boring parts. Not everything. Definitely not architecture.
>
> They're great at boilerplate: config files, test scaffolding, repetitive refactors. They're also great at sounding right while being wrong. I've accepted suggestions that compiled, passed lint, and still missed the point because I stopped paying attention.
>
> People I talk to tend to land in two camps. Some use it like autocomplete for chores and review every line. Others disable it after it keeps suggesting patterns they don't want. Both feel reasonable.
>
> The productivity metrics are slippery. GitHub can say Copilot users "accept 30% of suggestions," but acceptance isn't correctness, and correctness isn't value. If you don't have tests, you're basically guessing.

**Changes made:** Stripped the chatbot framing, significance inflation, promotional and -ing padding, rule-of-three and synonym cycling, false ranges, copula avoidance, em dashes/emojis/boldface/curly quotes, the formulaic "challenges" section, cutoff and hedging disclaimers, filler and persuasive framing, and the generic upbeat conclusion - then rebuilt the voice with varied rhythm and concrete detail.

## Reference

This skill is based on [Wikipedia:Signs of AI writing](https://en.wikipedia.org/wiki/Wikipedia:Signs_of_AI_writing), maintained by WikiProject AI Cleanup. The patterns documented there come from observations of thousands of instances of AI-generated text on Wikipedia.

Key insight from Wikipedia: "LLMs use statistical algorithms to guess what should come next. The result tends toward the most statistically likely result that applies to the widest variety of cases."
