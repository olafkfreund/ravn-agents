# Build the portal and serve it with nginx, proxying the API/WebSocket to the
# control plane. Used by docker-compose.
FROM node:22-slim AS build
WORKDIR /app
RUN corepack enable
COPY portal/package.json portal/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY portal/ ./
RUN pnpm build

FROM nginx:alpine
COPY docker/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/dist /usr/share/nginx/html
