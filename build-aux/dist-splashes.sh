#!/usr/bin/env bash
# Add information required for the splash texts to the configuration.
# This is required, as after dist, the git history is lost and therefore e.g. the number of commits don't work correctly.
# See also src/meson.build

export DIST="$1"
export SOURCE_ROOT="$2"

cd "$SOURCE_ROOT"

shopt -s globstar

number_of_commits=$(git log --oneline | wc -l)
number_of_tags=$(git tag | wc -l)
lines_of_rust=$(cat src/**/*.rs | wc -l)
lines_of_blueprint=$(cat data/resources/ui/*.blp | wc -l)
number_of_translated_languages=$(cat po/LINGUAS | wc -l)
number_of_translators=$(git log --format="%ae" po | sort | uniq | wc -l)
sed -i $DIST/src/config.rs.in \
  -e "s/@NUMBER_OF_COMMITS@/$number_of_commits/g"\
  -e "s/@NUMBER_OF_TAGS@/$number_of_tags/g"\
  -e "s/@LINES_OF_RUST@/$lines_of_rust/g"\
  -e "s/@LINES_OF_BLUEPRINT@/$lines_of_blueprint/g"\
  -e "s/@NUMBER_OF_TRANSLATED_LANGUAGES@/$number_of_translated_languages/g"\
  -e "s/@NUMBER_OF_TRANSLATORS@/$number_of_translators/g"
