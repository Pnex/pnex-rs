FROM python:3.14 as dump_req

WORKDIR /app

COPY pyproject.toml .
COPY uv.lock .

RUN pip install uv
RUN uv export --frozen --no-hashes --no-emit-project --no-dev --format requirements-txt -o requirements.txt
