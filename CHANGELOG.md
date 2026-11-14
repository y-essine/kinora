# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [4.1.1] - 2026-10-11

### Fixed

- Fetching Piped instance list timing out, preventing loading the feed.

## [4.1.0] - 2026-07-22

### Added

- Added a menu entry to mark videos as watched or unwatched.

## [4.0.4] - 2026-05-24

### Fixed

- Proper fix for videos from YouTube not showing up.

## [4.0.3] - 2026-05-22

### HOTFIX

- Fixed only very few videos from YouTube showing up.
  The workaround has negative performance implications, we will try to fix this in a subsequent MR.

### Fixed

- Restoring a backed up Pipeline-export failing in the Flatpak.
  Note that you need to do a new export for the fix to work, old exports remain broken.
- Very wide feed and channel page layouts having a single-column video view (again).

## [4.0.2] - 2026-04-29

### Fixed

- Fixed 4.0.1 release not containing vendored crates.

## [4.0.1] - 2026-04-28

### Fixed

- Very wide feed and channel page layouts having a single-column video view.

## [4.0.0] - 2026-04-22

### BREAKING

- The flatpak distributed on Flathub does not have the permission to run external commands, and therefore also not external players, by default.
  If you use external players, you will need to manually grant the permission, as detailed in the preferences dialog.

### Added

- Heavily reworked downloading functionality:
  Downloading is now built-in into Pipeline, backed by yt-dlp.
  You can also now play downloaded videos in the internal (or external) player.
  You can display the list of all videos you have downloaded in a new page.
- Introduced a history page.
- On a mobile layout, the downloads, history, and subscription entries were moved into an overlay sidebar.
- Add the ability to pin and unpin the sidebar on desktop mode.
- Improved the UI of the import and export dialogs.

### Changed

- Improved image loading performance for thumbnails and avatars.
- By default, the external player is now unset, informing the user to set it if he tries to play videos externally.

### Fixed

- Un-fullscreening a video always showing the sidebar, even though it was hidden before fullscreening.
- Fixed spaced in front of links being stripped in descriptions.

## [3.3.1] - 2026-03-18

### Fixed

- Bad performance with lots of subscriptions.
- Loading bar jumping around with many subscriptions.

## [3.3.0] - 2026-03-12

### Added

- Subscription import from Invidious.

### Fixed

- Video title updating when switching clicking on a video.

## [3.2.4] - 2026-02-24

### Added

- Colored progress bars.
- Added a small Easter egg.

### Fixed

- Subtitle settings not persisting.

### Changed

- Updated splash texts.

## [3.2.3] - 2026-02-15

### Added

- Pipeline can now use `yt-dlp` as a fallback to other providers.
- Pipeline can now use YouTube RSS feeds as a fallback to other providers.

## [3.2.2] - 2026-02-04

### HOTFIX

- Fixed videos failing to fetch from YouTube.

## [3.2.1] - 2026-02-02

### Added

- Remember video position when restarting a video.
- Option to use Piped as a fallback to fetching from YouTube, and vice versa.
  This is activated by default on new installations, and if you previously set to query from YouTube.

## [3.2.0] - 2026-01-29

### Added

- Keyboard shortcut F11 to fullscreen.
- Keyboard shortcut F9 to toggle the sidebar.
- Option to give a cookies file for video playback to confirm that you are not a bot.
- Option to set extractor arguments for internal video player.

### Changed

- Updated the internal video player to the newest version.
- Disabled creating error files for the backend.
- Updated the shortcuts dialog to the new one in libadwaita.

### Fixed

- Low-resolution thumbnails for YouTube.
- Rare crash when searching.
- Search keyboard shortcut not being usable in the search page.
- Focus changing when toggling the sidebar.

## [3.1.1] - 2025-10-22

### Fixed

- Channel video count being interpreted as subscriber count.
- Removing a video from the watch-later list sometimes duplicating the video in the list instead.

### Changed

- Removed the setup dialog in favor of a privacy notice dialog.

## [3.1.0] - 2025-10-15

### Added

- Option to directly connect to YouTube instead of proxying over Piped.
  Note that this option is enabled by default as currently all Piped instances seem to be down.

## [3.0.1] - 2025-08-15

### Fixed

- Fix playing Peertube videos using the external player if Clapper is used.
- Fix the setup window being slightly too wide in a narrow layout.
- Fix punctuation errors in some splash texts.

## [3.0.0] - 2025-08-07

This release is a major redesign of the application UI to be more intuitive to use, being more adaptive for both the desktop and mobile layout as well as more visually pleasing.
The release notes of this release will only contain some of the more noteworthy changes, many minor improvements are left out.

### BREAKING

- In case you are on an older device without support for GLES 3.0 or later, Pipeline will now use software rendering due to a GTK update.
  This will in particular break the internal video player.
  Please use an external video player instead (for example Clapper, which is also the internal video player we use).
  You can configure this, as well as make the external video player the default, in the preferences menu.

### Added

- Toast when unsubscribing to a channel, in order to undo the action if wanted.
- Keyboard shortcut for fullscreening a video.
- Filters can now be edited.
- Hovering over a video displays a button to bookmark the video, as well as a button to bring up the context menu (only works on a desktop layout).
- Export of data to either a Pipeline-specific database, NewPipe JSON file or YouTube CSV file.
- Option to disable YouTube or PeerTube providers.
- Option to use cairo software rendering in the settings, which may in some cases behave better compared to LLVMpipe software rendering on weak devices.

### Fixed

- Keyboard shortcuts for videos were only available when focused on the video widget, instead of the video page.
- Popover menus for videos and channels will now point to where the user clicked.
- Filters will now not be applied to channel pages anymore.
- When the window is unfullscreened via a keyboard shortcut, playing videos were still taking up the full window.
- Ensure all links, both in video descriptions and channel descriptions, are clickable.
- Fix some UI elements not being visible on a mobile layout.
- Ignored some Piped errors like internal server errors, which we can do nothing about.
- When manually managing Piped instances and only having one Piped instance, the 'automatically manage instances' was impossible to turn back on until another instance was added.
- When turning on automatically managed Piped instances, the updated list of instances is now directly fetched instead of only after a restart.

## [2.6.1] - 2025-06-28

### Fixed

- Some splash texts containing wrong information.
- The wide layout was used for windows with small height disregarding the width.

## [2.6.0] - 2025-06-21

### Added

- Keyboard shortcuts for changing volume and playback speed when playing a video.
- Keyboard shortcuts for seeking forward or backward, as well as to specific percentiles of a video.
- Toggle to hide the sidebar.
- Setup window to inform about Pipeline privacy and to import subscriptions.

### Fixed

- Error when Piped does not report any working instances.
- Avatars of channels being unset if no video was watched from this channel in a while.
- Multi-line video titles displaying weirdly.
- Status page that there are no subscriptions being displayed even if there are videos in the feed.

## [2.5.0] - 2025-06-14

### Added

- Random splash text when reloading the feed.
- Debug information into the about window.

### Fixed

- Some buttons being hidden on the video page in a narrow layout.
- Description of YouTube videos containing unescaped characters.
- Add videos to the watched list when closing the video while it is still being displayed.

## [2.4.1] - 2025-06-09

### Fixed

- The watch history was not correctly persisted.
- Filters which contained an ampersand where not correctly displayed in the UI.

## [2.4.0] - 2025-06-07

### Added

- New menu item for videos in the feed to filter out similar videos.
- Filter to remove already watched videos from the feed.

### Fixed

- Bug where 'hide videos from today' filter was wrong depending on the local timezone.

### Changed

- UI change for adding filters from the filter management dialog.

## [2.3.0] - 2025-05-28

### Added

- Option to hide videos which require payment; this is enabled by default as Pipeline cannot play those.

### Fixed

- Improved startup speed.
- Some extra information for YouTube videos not loading.
- Searches including YouTube videos with extremely many views not loading.

### Removed

- Support for importing data from versions earlier than 2.0.0.

## [2.2.3] - 2025-05-24

### Added

- When playing with the external player, click the thumbnail to spawn the external player again.

### Fixed

- Bug where video appears multiple times in watch later list.
- Videos starting playback with low resolution.

### Changed

- Disable the override to the GL renderer for new GTK versions, where the renderer was removed.

## [2.2.2] - 2025-04-22

### Added

- Option to automatically update the Piped instance list on every startup.

## [2.2.1] - 2025-04-20

## [2.2.0] - 2025-04-20

### Changed

- Make Piped requests distributed among all instances in the instance list.
  This should speed up refreshing the feed a lot.

### Fixed

- Videos with the same uploaded timestamp replacing each other in the watch-later list.

### Chores

- Updated the Piped API list to add more APIs.

## [2.1.1] - 2025-01-30

### Fixed

- Buttons and some video information being cut off in the video page on small height.

## [2.1.0] - 2024-12-05

### Changed

- Major UI improvements to the video page.
- Major UI improvements to the channel page.
- Minor UI improvements to duration indicators.

### Fixed

- Start with the watch-later list at the top instead of the bottom.
- Long video and channel names breaking window size on small screens.
- Wrong background color for video list and channel list.

## [2.0.3] - 2024-11-02

### Changed

- Improved import dialogs.
- Make the search bar wider.
- Improved error dialog.
- Show that the app is loading when starting the app.

### Fixed

- Panic for migrating watch-later list to the new version for some YouTube videos.
- Importing NewPipe videos not showing the file to import.

### Chores

- Updated to GNOME 47 runtimes.

## [2.0.2] - 2024-09-14

### HOTFIX

- Bad performance of the built-in video player.

### Added

- Ignore Piped error when opening a video.
- Don't show video title in player when not fullscreened.

## [2.0.1] - 2024-09-12

### Fixed

- Infinite loading on channels with no videos.
- Video not stopping when displaying channel in the subscriptions tab.
- Video player not centered on fullscreen when video aspect ratio does not match display aspect ratio.
- Slight redesign of channel page to fix video widgets growing in size when scrolling.
- Swiping not pausing video and exiting fullscreen.
- UI not fitting in window with certain widths.

## [2.0.0] - 2024-09-10

A complete rewrite of the application, in order to allow for easier maintenance, more features and a better user experience.

### Added

- Search for videos and channels.
- Play videos inside the application using [Clapper](https://github.com/Rafostar/clapper).
- Show toast when the video URL was copied.
- Import PeerTube subscriptions from NewPipe
- Allow for backup Piped instances.
- Option to not automatically refresh when started.
- Infinite scrolling of the feed.

### Fixed

- Many videos not showing up in the feed anymore. Workaround for an [upstream Piped issue](https://github.com/TeamPiped/Piped/issues/3761).

## Unreleased pre-2.0.0

### Fix

- Not grabbing focus for the entry when adding a subscription.

## [1.15.0] - 2024-05-28

### Fix

- Crash with certain channels.

### Chores

- Updated to GNOME 46 libraries.

## [1.14.5] - 2024-02-24

### HOTFIX

- Subscription names not showing up in the subscription list anymore.

## [1.14.4] - 2024-02-22

### Added

- Action to open video in the browser.

### Fixed

- Outdated window title
- Wrong offset of the reload-spinner in relation to the reload-button.

### Changed

- Updated selected Piped instance list.

## [1.14.3] - 2024-01-11

### Added

- "Livi" video player in the list of predefined video players.
- Option to hide all short videos.

## [1.14.2] - 2023-12-08

### Added

- Video title as tooltip of the video.
- Video title in the video information screen.
- Error handling when failing to add a subscription.
- Error handling when failing to play or download videos.

## [1.14.1] - 2023-11-11

### Added

- Keyboard shortcuts for actions.

### Fixed

- Video duration placed out-of-bounds if thumbnail does not load.

### Chores

- Updated dependencies.

## [1.14.0] - 2023-10-14

### Added

- A dropdown for some popular video players.
- A dropdown for some popular Piped APIs.

### Changed

- Removed buttons in the list in favor of a menu shown on right-click or long-press (touch-screen only). 
- Updated to GTK 4.12 and Libadwaita 1.4.
- Use GridView instead of ListView for all pages.
- Improvements regarding UI in the add-subscription dialog.
- The button on the empty feed page will now lead to the subscription page instead of directly adding a subscription.

### Fixed

- Inconsistent size of video thumbnails.

## [1.13.1] - 2023-08-24

### Fixed

- Copy video URL not working anymore.
- Missing accesibility labels.

## [1.13.0] - 2023-08-20

### Added

- Dialog showing video information including likes, dislikes (not for YouTube), views and video description.
- Show video duration on video thumbnails.

### Removed

- Removed Lbry support as it will have to shut down soon.

### Fixed

- File chooser dialog for importing videos not working.

[Unreleased]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/4.1.1...master
[4.1.1]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/4.1.0...4.1.1
[4.1.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/4.0.4...4.1.0
[4.0.4]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/4.0.3...4.0.4
[4.0.3]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/4.0.2...4.0.3
[4.0.2]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/4.0.1...4.0.2
[4.0.1]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/4.0.0...4.0.1
[4.0.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/3.3.1...4.0.0
[3.3.1]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/3.3.0...3.3.1
[3.3.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/3.2.4...3.3.0
[3.2.4]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/3.2.3...3.2.4
[3.2.3]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/3.2.2...3.2.3
[3.2.2]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/3.2.1...3.2.2
[3.2.1]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/3.2.0...3.2.1
[3.2.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/3.1.1...3.2.0
[3.1.1]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/3.1.0...3.1.1
[3.1.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/3.0.1...3.1.0
[3.0.1]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/3.0.0...3.0.1
[3.0.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/2.6.1...3.0.0
[2.6.1]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/2.6.0...2.6.1
[2.6.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/2.5.0...2.6.0
[2.5.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/2.4.1...2.5.0
[2.4.1]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/2.4.0...2.4.1
[2.4.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/2.3.0...2.4.0
[2.3.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/2.2.3...2.3.0
[2.2.3]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/2.2.2...2.2.3
[2.2.2]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/2.2.1...2.2.2
[2.2.1]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/2.2.0...2.2.1
[2.2.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v2.1.1...2.2.0
[2.1.1]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v2.1.0...v2.1.1
[2.1.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v2.0.4...v2.1.0
[2.0.4]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v2.0.3...v2.0.4
[2.0.3]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v2.0.2...v2.0.3
[2.0.2]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v2.0.1...v2.0.2
[2.0.1]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v2.0.0...v2.0.1
[2.0.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v1.15.0...v2.0.0
[1.15.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v1.14.5...v1.15.0
[1.14.5]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v1.14.4...v1.14.5
[1.14.4]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v1.14.3...v1.14.4
[1.14.3]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v1.14.2...v1.14.3
[1.14.2]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v1.14.1...v1.14.2
[1.14.1]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v1.14.0...v1.14.1
[1.14.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v1.13.1...v1.14.0
[1.13.1]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v1.13.0...v1.13.1
[1.13.0]: https://gitlab.com/schmiddi-on-mobile/pipeline/-/compare/v1.12.0...v1.13.0
