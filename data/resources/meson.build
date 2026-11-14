blueprints = custom_target('blueprints',
  input: files(
    'ui/video_list.blp',
    'ui/video_item.blp',
    'ui/window.blp',
    'ui/video_page.blp',
    'ui/finite_video_list.blp',
    'ui/subscriptions_list.blp',
    'ui/channel_item.blp',
    'ui/channel_page.blp',
    'ui/preferences_dialog.blp',
    'ui/filter_dialog.blp',
    'ui/filter_similar_dialog.blp',
    'ui/search_list.blp',
    'ui/search_list_item.blp',
    'ui/shortcuts.blp',
    'ui/error_dialog.blp',
    'ui/import_dialog.blp',
    'ui/export_dialog.blp',
    'ui/privacy_notice.blp',
    'ui/subscription_item.blp',
    'ui/player_sheet.blp',
    'ui/account_dialog.blp',
    'ui/video_row.blp',
    'ui/featured_banner.blp',
    'ui/home_view.blp',
  ),
  output: '.',
  command: [find_program('blueprint-compiler'), 'batch-compile', '@OUTPUT@', '@CURRENT_SOURCE_DIR@', '@INPUT@'],
)

# Resources
resources_conf = configuration_data()
resources_conf.set('app-id', application_id)
resources = gnome.compile_resources(
  'resources',
  configure_file(
    input: 'resources.gresource.xml.in',
    output: '@BASENAME@',
    configuration: resources_conf,
  ),
  gresource_bundle: true,
  install: true,
  install_dir: pkgdatadir,
  dependencies: [ appdata, blueprints ],
)
