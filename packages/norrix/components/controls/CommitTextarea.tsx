'use client'

import { useEffect, useRef, useState, type ComponentProps } from 'react'

import { Textarea } from '@norrix/ui/components/textarea'

type CommitTextareaProps = Omit<ComponentProps<typeof Textarea>, 'value' | 'onChange'> & {
  value: string
  delay?: number
  onCommit: (value: string, editSession: string | null) => void
}

let nextEditSession = 0

export function CommitTextarea({ value, delay = 360, onCommit, ...props }: CommitTextareaProps) {
  const [draft, setDraft] = useState(value)
  const timer = useRef<number | null>(null)
  const composing = useRef(false)
  const dirty = useRef(false)
  const draftRef = useRef(value)
  const editSession = useRef<string | null>(null)
  const onCommitRef = useRef(onCommit)
  onCommitRef.current = onCommit

  useEffect(() => {
    if (!dirty.current && !composing.current) {
      draftRef.current = value
      setDraft(value)
    }
  }, [value])

  useEffect(
    () => () => {
      if (timer.current !== null) window.clearTimeout(timer.current)
      if (dirty.current) onCommitRef.current(draftRef.current, null)
    },
    [],
  )

  const commit = (next: string, finish = false) => {
    if (timer.current !== null) window.clearTimeout(timer.current)
    timer.current = null
    if (!dirty.current) return
    if (!editSession.current) {
      nextEditSession += 1
      editSession.current = `layer-text-edit-${nextEditSession}`
    }
    onCommit(next, finish ? null : editSession.current)
    if (finish) {
      dirty.current = false
      editSession.current = null
    }
  }

  const schedule = (next: string) => {
    if (timer.current !== null) window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => commit(next), delay)
  }

  return (
    <Textarea
      {...props}
      value={draft}
      onChange={(event) => {
        const next = event.currentTarget.value
        dirty.current = true
        draftRef.current = next
        setDraft(next)
        if (!composing.current) schedule(next)
      }}
      onCompositionStart={() => {
        composing.current = true
      }}
      onCompositionEnd={(event) => {
        composing.current = false
        const next = event.currentTarget.value
        dirty.current = true
        draftRef.current = next
        setDraft(next)
        schedule(next)
      }}
      onBlur={() => commit(draft, true)}
    />
  )
}
