'use client'

import { useTranslation } from 'react-i18next'
import { X, AlertCircle } from 'lucide-react'
import { call } from '@/lib/backend'
import { commands, type Job, type PageErrorInfo } from '@koharu/bridge/protocol'
import { Button } from '@koharu/ui/components/button'
import { Accordion, AccordionItem, AccordionTrigger, AccordionContent } from '@koharu/ui/components/accordion'
import { Badge } from '@koharu/ui/components/badge'

interface PipelineSummaryProps {
  job: Job
  onDismiss: () => void
}

export function PipelineSummary({ job, onDismiss }: PipelineSummaryProps) {
  const { t } = useTranslation()
  const summary = job.pipeline_summary

  if (!summary) return null

  const { total_pages, successful_pages, failed_pages, errors } = summary

  // A page can fail at multiple stages; retry each distinct page once through
  // the full pipeline.
  const retryFailedPages = () => {
    const pageIds = [...new Set(errors.map((error) => error.page_id))]
    call(commands.process, { scope: 'pages', value: pageIds }, { operation: 'full' })
      .then(() => onDismiss())
      .catch(() => undefined)
  }

  return (
    <div className='fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4'>
      <div className='w-full max-w-2xl rounded-2xl border border-border bg-popover shadow-xl overflow-hidden'>
        <div className='flex items-center justify-between border-b p-4'>
          <h2 className='text-lg font-semibold'>{t('pipeline.summary.title')}</h2>
          <Button
            variant='ghost'
            size='icon'
            onClick={onDismiss}
            aria-label={t('common.dismiss')}
          >
            <X className='size-4' />
          </Button>
        </div>

        <div className='p-4 space-y-4'>
          {/* Summary stats */}
          <div className='grid grid-cols-3 gap-4'>
            <div className='rounded-lg border border-border p-4 text-center'>
              <div className='text-3xl font-bold text-foreground'>{total_pages}</div>
              <div className='text-sm text-muted-foreground'>{t('pipeline.summary.total')}</div>
            </div>
            <div className='rounded-lg border border-border p-4 text-center'>
              <div className='text-3xl font-bold text-green-600'>{successful_pages}</div>
              <div className='text-sm text-muted-foreground'>{t('pipeline.summary.successful')}</div>
            </div>
            <div className='rounded-lg border border-border p-4 text-center'>
              <div className='text-3xl font-bold text-destructive'>{failed_pages}</div>
              <div className='text-sm text-muted-foreground'>{t('pipeline.summary.failed')}</div>
            </div>
          </div>

          {/* Failed pages accordion */}
          {failed_pages > 0 && (
            <div className='space-y-2'>
              <h3 className='text-sm font-medium'>{t('pipeline.summary.failedPages')}</h3>
              <Accordion className='space-y-2'>
                {errors.map((error: PageErrorInfo, index: number) => (
                  <AccordionItem key={`${error.page_id}-${index}`} value={`${error.page_id}-${index}`}>
                    <AccordionTrigger className='flex items-center gap-2 p-3 bg-muted/50 rounded-lg'>
                      <AlertCircle className='size-4 text-destructive shrink-0' />
                      <div className='flex-1 min-w-0'>
                        <div className='flex items-center gap-2'>
                          <span className='font-medium truncate'>
                            {error.page_index ? t('pipeline.summary.pageNumber', { number: error.page_index }) : ''}
                            {error.page_name ? ` — ${error.page_name}` : error.page_id}
                          </span>
                          <Badge variant='outline' className='text-xs'>
                            {error.stage.charAt(0).toUpperCase() + error.stage.slice(1)}
                          </Badge>
                        </div>
                      </div>
                    </AccordionTrigger>
                    <AccordionContent className='p-3 space-y-3'>
                      <div className='grid grid-cols-2 gap-2 text-sm'>
                        <div>
                          <span className='text-muted-foreground'>{t('pipeline.summary.error')}:</span>
                        </div>
                        <div className='col-span-2 font-mono text-xs bg-muted p-2 rounded break-all'>
                          {error.error}
                        </div>
                        {error.output && (
                          <>
                            <div>
                              <span className='text-muted-foreground'>{t('pipeline.summary.output')}:</span>
                            </div>
                            <div className='col-span-2 font-mono text-xs bg-muted p-2 rounded break-all max-h-40 overflow-auto'>
                              {error.output}
                            </div>
                          </>
                        )}
                      </div>
                    </AccordionContent>
                  </AccordionItem>
                ))}
              </Accordion>
            </div>
          )}

          {/* Actions */}
          <div className='flex justify-end gap-2 pt-2 border-t'>
            <Button variant='outline' onClick={onDismiss}>
              {t('common.dismiss')}
            </Button>
            {failed_pages > 0 && (
              <Button onClick={retryFailedPages}>
                {t('pipeline.summary.retryFailed')}
              </Button>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}
