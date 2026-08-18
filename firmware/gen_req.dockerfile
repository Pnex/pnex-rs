FROM python:3.12 as dump_req

WORKDIR /app

COPY pyproject.toml .
COPY uv.lock .

RUN pip install uv
RUN uv pip compile pyproject.toml -o requirements.txt
