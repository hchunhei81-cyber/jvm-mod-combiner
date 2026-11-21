import { withSupabase } from 'npm:@supabase/server@^1'

const RESEND_API_KEY = Deno.env.get('RESEND_API_KEY')
const SUPABASE_URL = Deno.env.get('SUPABASE_URL')
const SERVICE_ROLE_KEY = Deno.env.get('SUPABASE_SERVICE_ROLE_KEY')

const handler = async (_request: Request): Promise<Response> => {
  // 查询待通知用户（last_notified_at 为空）
  const queryRes = await fetch(
    `${SUPABASE_URL}/rest/v1/waitlist?confirmed=eq.true&last_notified_at=is.null&select=id,email,name&limit=100`,
    {
      headers: {
        'apikey': SERVICE_ROLE_KEY,
        'Authorization': `Bearer ${SERVICE_ROLE_KEY}`,
      },
    }
  )

  if (!queryRes.ok) {
    return Response.json({ error: 'query failed' }, { status: 500 })
  }

  const users = await queryRes.json()
  if (users.length === 0) {
    return Response.json({ message: 'no pending users' })
  }

  // 构造 Resend batch 请求体（每封邮件可以有不同的收件人和内容）
  const batchPayload = users.map((u: any) => ({
    from: 'JVM Mod Combiner <onboarding@yourdomain.com>',  // 换成你验证过的域名
    to: u.email,
    subject: 'JVM Mod Combiner 项目更新',
    html: `<p>Hi ${u.name || 'there'}，</p><p>项目有新进展了，欢迎查看最新状态。</p>`,
  }))

  // 调用 Resend batch API（一次最多 100 封）
  const sendRes = await fetch('https://api.resend.com/emails/batch', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${RESEND_API_KEY}`,
    },
    body: JSON.stringify(batchPayload),
  })

  if (!sendRes.ok) {
    const err = await sendRes.text()
    return Response.json({ error: 'send failed', detail: err }, { status: 500 })
  }

  // 发信成功后，更新这些用户的 last_notified_at
  const ids = users.map((u: any) => u.id)
  const updateRes = await fetch(
    `${SUPABASE_URL}/rest/v1/waitlist?id=in.(${ids.join(',')})`,
    {
      method: 'PATCH',
      headers: {
        'apikey': SERVICE_ROLE_KEY,
        'Authorization': `Bearer ${SERVICE_ROLE_KEY}`,
        'Content-Type': 'application/json',
        'Prefer': 'return=minimal',
      },
      body: JSON.stringify({ last_notified_at: new Date().toISOString() }),
    }
  )

  if (!updateRes.ok) {
    // 邮件已发出，但标记失败，记录日志（下次会重复发，但至少不会丢）
    console.error('update last_notified_at failed')
  }

  return Response.json({ sent: users.length })
}

export default { fetch: withSupabase({ auth: ['secret:*'] }, handler) }