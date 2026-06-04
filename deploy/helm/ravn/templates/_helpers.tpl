{{/* Common labels applied to every object. */}}
{{- define "ravn.labels" -}}
app.kubernetes.io/part-of: ravn
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{- end -}}

{{/* The projected, audience-bound ServiceAccount token volume (#57). */}}
{{- define "ravn.tokenVolume" -}}
- name: ravn-token
  projected:
    sources:
      - serviceAccountToken:
          path: token
          audience: {{ .Values.controlPlane.tokenAudience | quote }}
          expirationSeconds: 3600
{{- end -}}

{{/* Shared agent env (control-plane ingest + identity). */}}
{{- define "ravn.commonEnv" -}}
- name: RAVN_INGEST_URL
  value: {{ .Values.controlPlane.ingestUrl | quote }}
- name: RAVN_SA_TOKEN_FILE
  value: {{ .Values.controlPlane.tokenPath | quote }}
- name: RAVN_CLUSTER
  value: {{ .Values.cluster | quote }}
- name: RAVN_LOG
  value: {{ .Values.logLevel | quote }}
{{- end -}}
