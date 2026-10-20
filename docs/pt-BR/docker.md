# Usando o Guará com Docker

[English](../docker.md) · **Português**

A imagem traz o terminal `guara` e o `smartctl`. Ela serve em três casos:

1. **Servidor ou NAS Linux sem desktop**, com o disco danificado conectado.
2. **Arquivos de imagem forense** (`.img`, E01, AFF4) em qualquer sistema com Docker, porque não precisa acessar dispositivo.
3. **Ambiente reproduzível** para scripts e CI.

**Não** é o caminho recomendado para a maioria das pessoas. No desktop, use os pacotes de [Releases](../../../../releases).

## Limites

- **Dispositivos brutos só funcionam em hospedeiros Linux.** No Windows e no macOS o Docker roda dentro de uma máquina virtual e não repassa discos físicos. Use um arquivo de imagem.
- **A interface gráfica não é servida pelo container.** O `guara ui` escuta só em `127.0.0.1` e só aceita esse `Host`, de propósito; abrir isso para fora enfraqueceria a proteção, então não faz parte da imagem.
- Arquivos gravados numa pasta montada pertencem ao root, a menos que você passe `--user` (possível ao trabalhar com arquivos de imagem).

## Construir

```bash
docker build -t guara .
```

## Arquivos de imagem (qualquer sistema)

Monte a pasta da imagem somente para leitura e outra pasta para os resultados:

```bash
docker run --rm \
  -v "$PWD/imagens:/imagens:ro" \
  -v "$PWD/saida:/saida" \
  --user "$(id -u):$(id -g)" \
  guara scan /imagens/disco.img --session /saida/s.json --list

docker run --rm \
  -v "$PWD/imagens:/imagens:ro" -v "$PWD/saida:/saida" \
  --user "$(id -u):$(id -g)" \
  guara recover /saida/s.json /saida/recuperados
```

## Disco físico (hospedeiro Linux)

Passe o dispositivo **somente para leitura** com a opção `:r`. O Docker passa a bloquear escritas nele no nível do kernel, além de o Guará nunca gravar na origem:

```bash
docker run --rm \
  --device=/dev/sdb:/dev/sdb:r \
  -v /mnt/outro-disco/saida:/saida \
  guara scan /dev/sdb --session /saida/s.json --list
```

- O destino precisa estar em **outro disco** que não seja o `/dev/sdb`.
- Ler dispositivo bruto exige root dentro do container, então não use `--user` aqui.
- Verificado localmente com um dispositivo de loop (Linux): `inspect`, `scan` e `recover` leem através de `--device=...:r`, e uma tentativa de escrita (`dd of=/dev/loop0`) falha com `Operation not permitted`. Dispositivos de loop ficam ocultos no `guara devices`, como outros dispositivos virtuais, então informe o caminho explicitamente; um pendrive real aparece na lista.
- O SMART depende do hospedeiro, do controlador e do dispositivo. `--cap-add=SYS_RAWIO` é o complemento usual quando só `--device` não basta, mas isso **não** foi verificado com um dispositivo de loop, que não tem dados SMART.
- Evite `--privileged`: ele dá ao container acesso a todos os dispositivos.

## Outros comandos

Tudo da [referência do terminal](cli.md) funciona do mesmo jeito, por exemplo:

```bash
docker run --rm --device=/dev/sdb:/dev/sdb:r guara devices
docker run --rm --device=/dev/sdb:/dev/sdb:r guara health /dev/sdb
```
