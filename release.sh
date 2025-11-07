#!/bin/bash
set -eux

git_tag=$(git describe --tags)
pypi_version=$(echo "$git_tag" | sed 's/^v//')
cp pyproject.toml pyproject.toml.bak
sed -b 's/^version = "0\.0\.0"$/version = "'"${pypi_version}"'"/' < pyproject.toml.bak > pyproject.toml

poetry build

mv pyproject.toml.bak pyproject.toml
