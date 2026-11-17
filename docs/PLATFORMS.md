# One app on every platform (plan and state)

The same four sections everywhere (UI-SPEC.md): Trace or drop, Journal, Spaces, Settings. A platform adds only what
its device has. Every device is a node: it writes into its own copy and the synchronisation carries it (FEDERATION.md).

| Platform | State | What is its own |
|---|---|---|
| Node page (Home Assistant, Linux, any node) | built, four sections, golden paths tested in CI | managing the node (people, connections, peers, updates) |
| Mac | built, four sections, its own node, peer sync, join requests answered by notification | recording meetings (everyone told and agreeing); folders connected to a space, a cloud folder told to stay on the Mac |
| iPhone | built, four sections | a window of thirty days; Apple Health read and proposals written to it; Siri, Spotlight |
| iPad | the iPhone app, a sidebar instead of tabs (`.sidebarAdaptable`) | the same as the iPhone, a larger window |
| Apple Watch | a microphone and the state of the machines; the note goes through the iPhone | complication: the state and today's count |
| CarPlay | plan | see below |
| Windows | plan | see below |
| Linux desktop (GTK4) | plan | see below |

## CarPlay (plan)

Only « Trace or drop » by voice, hands-free: Siri « Dicter dans MemCP », then the note is read back once and filed.
Apple lets a CarPlay app exist only in its listed categories, each with an entitlement Apple grants on request; a voice
note fits « voice-based conversational » if Apple offers it for this use, otherwise Siri on the iPhone is the way in the
car (App Shortcuts work through CarPlay's Siri). To check with Apple before any code: the category and the entitlement.

## Windows (plan)

A native app (WinUI 3) around the same node (`memcp`, built for Windows in CI), the four sections, tray icon for the
microphone, Windows Hello for the lock. Engines: Windows speech recognition by default, Whisper in the app, a local
model through Foundry Local when present (LOCAL-TOOLS.md). Folders: a folder under OneDrive that is « online-only » keeps
placeholders; connecting one says, in OneDrive's words, to right-click the folder › « Always keep on this device ». The
same for Google Drive for desktop (« Available offline ») and Dropbox (« Make available offline »).

## Linux desktop (GTK4, plan)

A GTK4 / libadwaita app (Rust, gtk4-rs) around the same node, the four sections in an `AdwNavigationSplitView`, the
microphone through PipeWire, the lock through the session's password (polkit). Engines: Whisper in the app; a local
model through Ollama when present. Folders: a folder mounted by a cloud client (rclone, GNOME Online Accounts) is said
to need a full local copy (`--vfs-cache-mode full` or the client's « keep offline »).
