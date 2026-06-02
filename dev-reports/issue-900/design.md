# Design Note

Issue #900 focuses on workspace/artifact false positives, especially `output.csv`.

Plan:

- keep the existing `WorkspacePolicy` path-classification model,
- narrow data-output obligation inference so coding tasks that merely read/write CSV/JSONL do not create standalone `DataOutput` obligations,
- preserve explicit standalone data-output tasks such as `Generate output.csv with columns ...`.

