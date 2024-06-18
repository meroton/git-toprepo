#!/bin/bash
set -eux
{
    export POETRY_VIRTUALENVS_PATH=.virtualenvs/lint
    poetry env use python
    poetry install --sync --only lint
    poetry run black git_toprepo*.py
    poetry run ruff check git_toprepo*.py
}
{
    export POETRY_VIRTUALENVS_PATH=.virtualenvs/test
    poetry env use python
    poetry install --sync --only main,test
    poetry run pytest
}
poetry build

cat <<EOF
Consider to run
  poetry update
  poetry lock
EOF
