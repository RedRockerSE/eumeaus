I want to build Eumeaus - a commercial, professional OSINT tool.

Context you should have:
- Language/runtime: Not decided yet. But the idea for the final project is a desktop-app with a GUI that enables the user to display and edit collected data, display and edit graphs with connection found between entities etc. This app will probably need some kind of engine doing the heavy lifting. The engine (or backend) would preferrebly be extendable adding plugins for specific tasks (i.e getting username-info like https://github.com/sherlock-project/sherlock does).
- Target users: law enforcements investigators, attorneys and other law practioners, journalists and everyone needing to do OSINT work.
- Runs where: Not decided yet but see the Language/runtime-section above for clues.
- Must integrate with: Online open-source sources, API's etc. Some integration may need accountinfo, accesstokens etc 
- Hard constraints: Not known currently
- Explicitly out of scope: Client-server solution at this point.
- Definition of done for v1: Backend/engine with working plugin-system as well as a proof-of-concept plugin that implements the same functionality as: https://github.com/sherlock-project/sherlock. Persisting data collected.

Interview me in detail using the AskUserQuestion tool. Ask about technical
implementation, data model, error handling, edge cases, testing strategy, and
tradeoffs. Don't ask obvious questions — dig into the hard parts I might not have
considered. Challenge my assumptions where you disagree.

Keep interviewing until we've covered everything, then write a complete spec to
SPEC.md containing:
1. Problem statement and non-goals
2. Architecture: the 3-5 modules and what each owns
3. Public interfaces (function/class signatures, CLI surface, file formats)
4. Data model / on-disk formats
5. Error handling and failure modes
6. Test strategy, including the end-to-end check that proves v1 works
7. Milestones, ordered, each independently verifiable
8. Open questions I still need to decide

Do not write any implementation code yet.
