# App Review Notes — FlowFile 1.0

FlowFile is a local multi-pane file manager for macOS. No account or sign-in is required.

## Review steps

1. Launch the app.
2. Click “选择并授权…” in the blue onboarding banner.
3. Select a test folder containing files and subfolders. The app uses the standard `NSOpenPanel` and stores an app-scoped security-scoped bookmark locally.
4. Switch between single, vertical two-pane, horizontal two-pane, and four-pane layouts using the top toolbar.
5. Select a file and use copy/move operations between panes. Test data is not transmitted off the Mac.
6. Press Space on a supported file to open the in-app/Quick Look preview.
7. Use the “＋ 授权文件夹” toolbar button to grant access to another test folder.

## Sandbox and data handling

- App Sandbox is enabled.
- File access is limited to the app container and folders explicitly selected by the reviewer through the system picker.
- Persistent access uses app-scoped security-scoped bookmarks stored only inside the app container.
- The Mac App Store build does not execute scripts or binaries, launch Terminal, mount SMB shares through scripts, request administrator privileges, or use an independent update mechanism.
- The app contains no analytics, advertising, tracking, cloud upload, or user-account SDK.

## Network volumes

For a network volume, first connect it in Finder, then choose the mounted folder using “＋ 授权文件夹”. The App Store build does not collect or store network credentials.

No demo account or special hardware is required.
