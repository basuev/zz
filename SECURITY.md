# Security policy

## Reporting a vulnerability

Do not open a public issue for a vulnerability that could expose prompt contents, workspace paths, local files, or release credentials.

Use GitHub's private vulnerability reporting for this repository. Include the affected version or commit, operating system, reproduction steps, impact, and any suggested mitigation. Remove secrets and proprietary prompt or source content from the report.

If private vulnerability reporting is unavailable, contact the repository owner through the email address on the owner's GitHub profile and ask for a private reporting channel. Do not send exploit details in the first message.

## Scope

Security-sensitive areas include:

- replacement of files supplied through the external-editor interface;
- permissions and lifecycle of prompt history and recovery drafts;
- path handling and context references;
- terminal escape and paste handling;
- release artifacts and the release workflow.

Only the latest release and the current default branch receive security fixes while the project is pre-1.0.
