FROM python:3.11-slim
WORKDIR /app
COPY services/voiceprint_gateway/requirements.txt /app/requirements.txt
RUN pip install --no-cache-dir -r requirements.txt && useradd --uid 10001 --create-home listener
COPY services/voiceprint_gateway /app/services/voiceprint_gateway
RUN mkdir /data && chown listener:listener /data
USER listener
ENV DATABASE_PATH=/data/voiceprints.sqlite3
EXPOSE 8080
CMD ["uvicorn", "services.voiceprint_gateway.app:production_app", "--factory", "--host", "0.0.0.0", "--port", "8080", "--workers", "1", "--no-access-log"]
