Use the Crossthreads MCP tools to recall relevant prior work before continuing.

Topic (optional): $ARGUMENTS

1. Call `crossthreads_recall` with a question describing the current task (or the
   topic above, if given) to get a digest of relevant past sessions across all my
   AI coding tools.
2. If you're about to solve something that may have been solved before, also run
   `crossthreads_search` (mode: hybrid) and skim the top results.
3. To see what's still outstanding, call `crossthreads_open_loops`; to match my
   conventions, `crossthreads_how_i_work`; for past decisions,
   `crossthreads_decision_log`. (These higher-level tools use my configured
   model; if none is set they'll say so.)
4. Summarize what's relevant to the task at hand and how it applies — do not
   paste raw results.

Recall proactively: consult these tools before planning non-trivial work, even
when I don't ask. It's cheap and avoids re-deriving past decisions.

If a tool errors with a connection problem, the Crossthreads daemon isn't
running; tell me to start it with `crossthreads-up`.
