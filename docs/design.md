# Runner self-hosted no PC do dono · pesquisa e opções

Estado: **proposta, aguardando aprovação do dono.** Nada foi registrado nem mudado no GitHub.

## O problema

A CI (trabalhos `linux` e `windows`) demora e atrasa o ciclo de PRs. O PC do dono (Windows, SolidWorks, cache do cargo quente) pode rodar os jobs quando estiver parado, sem atrapalhar quando estiver em uso, em trânsito ou com pouca bateria. Três modos:

1. **Inativo há ≥ 5 min:** roda tudo o que chegar.
2. **Inativo há < 5 min:** popup pergunta "rodar aqui ou mandar para o GitHub?".
3. **Desativado:** nunca roda.

## Fatos levantados

- **Repositório privado** (`gh repo view`: `PRIVATE`). O risco clássico (PR de fork de estranho rodando código no PC) é bem menor que num público, mas a recomendação do GitHub continua: runner self-hosted não é sandbox; quem escreve no repositório roda código no PC. Em repositório privado, PRs de fork só rodam se a opção "Run workflows from fork pull requests" estiver ligada; manter desligada.
- **`runs-on` com lista é E, não OU.** `runs-on: [self-hosted, pc-dono]` exige um runner com todas as labels. Não existe "use o PC, senão o GitHub" nativo.
- **Job sem runner fica na fila até 24 h** e então falha. `timeout-minutes` só conta depois que o job começa; não protege a fila. Logo, mandar um job para o PC e o PC sumir = job preso. O roteamento tem de ser decidido **antes**, com o PC comprovadamente disponível.
- **Uma vez atribuído, o runner não recusa o job.** O popup tem de acontecer antes da atribuição, nunca dentro do job.
- **API de runners:** `GET /repos/{owner}/{repo}/actions/runners` devolve `status` (`online`/`offline`) e `busy`. Exige token com permissão de administração do repositório; o `GITHUB_TOKEN` não basta. Precisa de um PAT fine-grained (só este repositório, "Administration: read") guardado como secret.
- **`--ephemeral`:** o runner aceita um job e se desregistra; limpa estado entre jobs. Bom para segurança, mas exige re-registro a cada job (token de registro via API). Alternativa: runner persistente com `--disableupdate` não, só o serviço normal com diretório de trabalho limpo pelo próprio workflow.
- **Inatividade:** `GetLastInputInfo` (user32) dá o tick do último teclado/mouse; idle = `GetTickCount64() - dwTime`. Só funciona na sessão interativa do usuário: o agente tem de rodar como app de bandeja na sessão do dono, não como serviço (serviço roda na sessão 0 e não vê a entrada nem mostra popup).
- **Energia:** `GetSystemPowerStatus` dá `ACLineStatus` (0 bateria, 1 tomada) e `BatteryLifePercent`. Também há eventos `WM_POWERBROADCAST` para suspensão/retomada.
- **Popup com tempo limite:** `MessageBoxTimeout` (não documentada) ou, melhor, uma janela própria/notificação toast com contagem regressiva. Padrão sem resposta: **GitHub** (ele pode estar longe do PC).

## Opções

### A · Job roteador + agente que liga e desliga o runner (recomendada)

- Um job `rota` em `ubuntu-latest` consulta a API: se existir runner `pc-dono` `online` e não `busy`, devolve `["self-hosted","pc-dono"]`; senão `"windows-latest"`. Os jobs pesados usam `runs-on: ${{ fromJSON(needs.rota.outputs.runner) }}`.
- O agente de bandeja no PC controla o processo do runner:
  - modo 3 ou bateria → runner parado (offline) → tudo vai para o GitHub;
  - idle ≥ 5 min na tomada → runner ligado;
  - idle < 5 min → runner parado; o agente consulta a API a cada ~15 s por execuções novas; ao ver uma, mostra o popup (45 s, padrão GitHub). "Sim" liga o runner; o `rota` espera até ~60 s pelo runner ficar online antes de escolher o GitHub.
- Proteções: `timeout-minutes` nos jobs; o agente nunca desliga o runner com job em andamento (`busy`).
- Custo: ~15–30 s a mais por execução no `rota`; um PAT para guardar.
- Risco residual: corrida pequena (runner fica offline entre a consulta e a atribuição, ex. PC dorme) → job preso. Mitigação: o agente segura o modo de suspensão do Windows (`SetThreadExecutionState`) enquanto o runner estiver online.

### B · Variável do repositório escrita pelo agente

O agente grava `vars.RUNNER_WINDOWS` (`windows-latest` ou `["self-hosted","pc-dono"]`) conforme o modo. Sem job roteador. Mais simples, mas não atende o modo 2 (a decisão é tomada no enfileiramento, sem popup) e uma variável velha (PC desligou sem avisar) prende jobs por 24 h. Descartada como solução final; serve de primeiro passo manual.

### C · Escolha manual por `workflow_dispatch`

Input "onde rodar". Zero automação, zero risco, nenhum dos três modos. Descartada.

### D · Rodar sempre nos dois e aproveitar o primeiro

Duplica custo e não resolve o job preso quando o PC some. Descartada.

## Recomendação

**Opção A**, em fatias:

1. Só o trabalho `windows` (o PC é Windows; `linux` fica no GitHub por ora, ou depois via WSL/Docker).
2. Agente sem popup: modos 1 e 3 + regra da bateria. Medir o ganho de tempo.
3. Popup do modo 2.

Depende de aprovação do dono para: criar o PAT fine-grained e o secret, registrar o runner com a label `pc-dono`, e confirmar que "Run workflows from fork pull requests" está desligada.

## Decisões do dono (2026-10-30)

- **Forma:** ícone na bandeja do sistema (canto inferior direito), como o Tailscale. Sem janela, só o menu do ícone.
- **Bateria:** é uma opção do menu ("Na bateria, mandar para o GitHub"), não uma regra fixa.
- **Popup:** 45 s; sem resposta, vai para o GitHub.
- **Linguagem:** Rust.

- **Escopo:** projeto próprio, público e open source, multiplataforma (Linux é prioridade de acessibilidade), com release de binários; entra aqui como submodule, na infra de desenvolvimento. Motivação: rodar a CI na própria máquina quando o dono quer, automaticamente, e manter o Actions quando não.

Ainda em aberto: a opção A em si, e a aprovação para o PAT, o registro do runner e a conferência de PRs de fork.
