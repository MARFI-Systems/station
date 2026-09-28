import { isEmailEntityWritable } from '@core/email-link/entity-capability';
import type { EntityData } from '@entity';
import { useNonPrimaryEmailLinkIdHeader } from '@queries/email/link';
import { blockSenderWithToast } from '@queries/email/thread';
import type { EntityActionListState } from './entity-action-context';

export const makeBlockSenderAction = () => {
  const toHeaderLinkId = useNonPrimaryEmailLinkIdHeader();

  // Read-only (currently: Microsoft) links can't take a blocking filter
  // either — same frontend UX guard shared across keyboard, context menu,
  // and bulk multi-select through this factory.
  const canExecute = (entity: EntityData): boolean => {
    return (
      entity.type === 'email' &&
      !!entity.senderEmail &&
      isEmailEntityWritable(entity)
    );
  };

  const execute = async (entities: EntityData[]) => {
    for (const entity of entities) {
      if (
        entity.type !== 'email' ||
        !entity.senderEmail ||
        !isEmailEntityWritable(entity)
      ) {
        continue;
      }
      // The block creates a Gmail filter on one linked account, so it has to
      // target the inbox the thread arrived in.
      await blockSenderWithToast(
        entity.senderEmail,
        toHeaderLinkId(entity.linkId)
      );
    }
  };

  const executeWithSoup = async (
    entities: EntityData[],
    _soup: EntityActionListState
  ) => {
    await execute(entities);
  };

  return { canExecute, execute, executeWithSoup };
};
